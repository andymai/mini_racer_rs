# frozen_string_literal: true

# Runs mini_racer's own test suite, unmodified, against mini_racer_rs (or, with
# --reference, against the real mini_racer 0.22.1) and checks the results
# against the ledger for this Ruby and platform.

require "fileutils"
require "json"
require "optparse"
require "rbconfig"
require_relative "ledger"

options = {
  mode: "candidate",
  seed: Random.new_seed % 65_536,
  strict: false,
  scripts: false,
  weekly: false,
  ledger: File.join(__dir__, "mini_racer.status.toml"),
  scripts_ledger: File.join(__dir__, "scripts.status.toml"),
}
OptionParser.new do |opts|
  opts.banner = "usage: run.rb [options]"
  opts.on("--reference", "Run the real mini_racer 0.22.1 from tools/reference") { options[:mode] = "reference" }
  opts.on("--seed N", Integer) { |seed| options[:seed] = seed }
  opts.on("-n PATTERN", "Only tests matching PATTERN (minitest -n)") { |pattern| options[:filter] = pattern }
  opts.on("--files LIST", "Comma-separated test files relative to vendor/mini_racer") do |list|
    options[:files] = list.split(",")
  end
  opts.on("--out PATH", "Write one JSON result per test") { |path| options[:out] = path }
  opts.on("--strict", "Fail unless every result matches the ledger") { options[:strict] = true }
  opts.on("--scripts", "Run upstream's standalone scripts instead of the suite") { options[:scripts] = true }
  opts.on("--weekly", "With --scripts, include the scripts marked weekly") { options[:weekly] = true }
  opts.on("--ledger PATH") { |path| options[:ledger] = path }
end.parse!

ROOT = UpstreamLedger::ROOT
UPSTREAM = UpstreamLedger::UPSTREAM
LEG = UpstreamLedger.leg
WORK = File.join(ROOT, "tmp", "upstream")
CLEAN_EXITS = [0, 1].freeze
FileUtils.mkdir_p(WORK)

unless File.exist?(File.join(UPSTREAM, "test", "test_helper.rb"))
  abort "vendor/mini_racer is empty; run `git submodule update --init`"
end

def child_env(mode, report)
  reference = mode == "reference"
  env = {
    "BUNDLE_GEMFILE" => File.join(ROOT, reference ? "tools/reference/Gemfile" : "Gemfile"),
    "RUBYOPT" => "-rbundler/setup -r#{File.join(__dir__, reference ? "preload_reference.rb" : "preload.rb")}",
    "MINI_RACER_RS_UPSTREAM_REPORT" => report,
    "MINI_RACER_RS_UPSTREAM_PARENT" => Process.pid.to_s,
  }
  env["BUNDLE_PATH"] = File.join(ROOT, "tools/reference/vendor/bundle") if reference
  env
end

# The child must not inherit this process's Bundler setup.
def spawn_clean(env, *args, **opts)
  spawner = -> { Process.spawn(env, *args, **opts) }
  defined?(Bundler) ? Bundler.with_unbundled_env(&spawner) : spawner.call
end

def native_stacks(pid)
  if system("command -v eu-stack >/dev/null 2>&1")
    `eu-stack -p #{pid} 2>&1`
  elsif system("command -v sample >/dev/null 2>&1")
    `sample #{pid} 1 2>&1`
  else
    "no eu-stack or sample available"
  end
end

def read_events(path, offset)
  return [[], offset] unless File.exist?(path)

  File.open(path) do |f|
    f.seek(offset)
    complete = f.read.rpartition("\n").first
    events = complete.split("\n").reject(&:empty?).map { |line| JSON.parse(line) }
    [events, offset + complete.bytesize + (complete.empty? ? 0 : 1)]
  end
end

def suite_command(files, options, excluded)
  code = files.map { |file| "require #{File.join(UPSTREAM, file).inspect}" }.join("; ")
  args = [RbConfig.ruby, "-Itest", "-e", code, "--", "--seed", options[:seed].to_s]
  args += ["-n", options[:filter]] if options[:filter]
  unless excluded.empty?
    names = excluded.map { |id| Regexp.escape(id.split("#", 2).last) }
    args += ["-e", "/\\A(?:#{names.join("|")})\\z/"]
  end
  args
end

def kill_overdue(pid, id, report, limit)
  Process.kill("USR2", pid)
  sleep 2
  stacks = native_stacks(pid)
  Process.kill("KILL", pid)
  _, status = Process.wait2(pid)
  threads = File.exist?("#{report}.threads") ? File.read("#{report}.threads") : ""
  File.write(File.join(WORK, "#{id.tr("/#", "__")}.timeout.txt"), "#{threads}\n\n#{stacks}")
  [status, { "status" => "timeout", "message" => "exceeded #{limit}s" }]
end

# One process runs the files, as upstream's Rake task does. A hung or crashed
# test is recorded, and the run resumes without the tests already finished.
def run_suite(options, ledger)
  files = options[:files] || (Dir.chdir(UPSTREAM) { Dir["test/*_test.rb"] } + ["test/test_object_identity_fix.rb"]).sort
  results = {}
  attempt = 0
  loop do
    attempt += 1
    before = results.size
    stem = File.join(WORK, "#{options[:mode]}-#{LEG}-#{options[:seed]}-#{attempt}")
    report = "#{stem}.events.jsonl"
    log = "#{stem}.log"
    FileUtils.rm_f(report)
    pid = spawn_clean(child_env(options[:mode], report), *suite_command(files, options, results.keys),
                      chdir: UPSTREAM, out: log, err: %i[child out],)

    started = {}
    offset = 0
    status = nil
    until status
      _, status = Process.wait2(pid, Process::WNOHANG)
      events, offset = read_events(report, offset)
      events.each do |event|
        if event["event"] == "start"
          started[event["id"]] = event["at"]
        else
          started.delete(event["id"])
          results[event["id"]] = event.slice("status", "message", "time")
        end
      end
      next if status

      id, at = started.first
      limit = UpstreamLedger.timeout_for(ledger[id])
      if id && Process.clock_gettime(Process::CLOCK_MONOTONIC) - at > limit
        status, results[id] = kill_overdue(pid, id, report, limit)
        started.delete(id)
      end
      sleep 0.2
    end

    abort "upstream harness: the guard tripped; see #{log}" if status.exitstatus == 97

    reason = status.signaled? ? "signal #{status.termsig}" : "exit #{status.exitstatus}"
    started.each_key { |id| results[id] = { "status" => "crash", "message" => reason } }
    break if status.exited? && CLEAN_EXITS.include?(status.exitstatus)

    if results.size == before
      warn File.read(log).lines.last(40).join
      abort "upstream harness: the suite process died outside any test (#{status.inspect}); see #{log}"
    end
  end
  abort "upstream harness: the filter #{options[:filter].inspect} selected no tests" if results.empty?
  results
end

def wait_with_limit(pid, limit)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  loop do
    _, status = Process.wait2(pid, Process::WNOHANG)
    return status if status

    Process.kill("KILL", pid) if Process.clock_gettime(Process::CLOCK_MONOTONIC) - started > limit
    sleep 0.2
  end
end

def run_scripts(options)
  entries = Tomlrb.load_file(options[:scripts_ledger]).fetch("scripts")
  done = Tomlrb.load_file(UpstreamLedger::MILESTONES).fetch("milestones").select { |_, m| m["done"] }.keys
  entries.filter_map do |script, entry|
    next if entry["weekly"] && !options[:weekly]

    log = File.join(WORK, "#{File.basename(script, ".rb")}-#{LEG}.log")
    env = child_env("candidate", File.join(WORK, "scripts.events.jsonl"))
    pid = spawn_clean(env, RbConfig.ruby, script, chdir: UPSTREAM, out: log, err: %i[child out])
    status = wait_with_limit(pid, entry.fetch("timeout_s"))
    outcome = status.exited? ? "exit #{status.exitstatus}" : "signal #{status.termsig}"
    ok = status.exited? && status.exitstatus == entry.fetch("expected_exit")
    enforced = done.include?(entry.fetch("milestone"))
    verdict = if ok then "ok"
              elsif enforced then "FAIL"
              else "recorded (#{entry["milestone"]} pending)"
              end
    puts "#{script.ljust(28)} #{outcome.ljust(10)} #{verdict}"
    "#{script}: #{outcome}, expected exit #{entry["expected_exit"]}" if enforced && !ok
  end
end

def ledger_problems(ledger, results, options)
  known = UpstreamLedger.milestones
  problems = ledger.flat_map { |id, entry| UpstreamLedger.malformed(id, entry, known) }
  results.each do |id, result|
    entry = ledger[id]
    next problems << "#{id}: not in the ledger (#{result["status"]})" unless entry

    verdict = UpstreamLedger.verdict(UpstreamLedger.effective(entry, LEG)["status"], result["status"])
    problems << "#{id}: #{verdict}" if verdict
  end
  return problems if options[:filter]

  (ledger.keys - results.keys).each do |id|
    next if options[:files] && !options[:files].include?(id.split("#", 2).first)

    problems << "#{id}: in the ledger but did not run"
  end
  problems
end

if options[:scripts]
  problems = run_scripts(options)
  abort problems.join("\n") unless problems.empty?
  exit
end

ledger = UpstreamLedger.load(options[:ledger])
results = run_suite(options, ledger)

out = options[:out] || File.join(WORK, "#{options[:mode]}-#{LEG}-#{options[:seed]}.jsonl")
lines = results.sort.map do |id, result|
  JSON.generate(result.merge("id" => id, "mode" => options[:mode], "leg" => LEG, "seed" => options[:seed]))
end
File.write(out, "#{lines.join("\n")}\n")

counts = results.values.map { |result| result["status"] }.tally.sort.map { |status, n| "#{n} #{status}" }
puts "#{options[:mode]} #{LEG} seed #{options[:seed]}: #{results.size} tests, #{counts.join(", ")}"
puts "results: #{out}"
exit unless options[:strict]

problems = ledger_problems(ledger, results, options)
if problems.empty?
  puts "ledger: every result matches #{File.basename(options[:ledger])} for #{LEG}"
else
  puts problems.sort
  abort "ledger: #{problems.size} mismatch(es) for #{LEG}"
end
