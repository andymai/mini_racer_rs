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
# Longest quiet stretch with no test in flight (loading files, between tests,
# at exit) before the suite process counts as hung.
SUITE_IDLE_LIMIT = 300
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
    "MINI_RACER_RS_UPSTREAM_LIB" => nil,
  }
  if reference
    env["BUNDLE_PATH"] = File.join(ROOT, "tools/reference/vendor/bundle")
  elsif ENV["BUNDLE_WITHOUT"]
    # with_unbundled_env drops every BUNDLE_* variable, and CI installs some
    # groups only; the child must ask Bundler for the same groups.
    env["BUNDLE_WITHOUT"] = ENV["BUNDLE_WITHOUT"]
  end
  env
end

# The child must not inherit this process's Bundler setup. Its own process group
# lets the watchdog kill every process a hung test started.
def spawn_clean(env, *args, **opts)
  spawner = -> { Process.spawn(env, *args, pgroup: true, **opts) }
  defined?(Bundler) ? Bundler.with_unbundled_env(&spawner) : spawner.call
end

def kill_group(pid)
  Process.kill("KILL", -pid)
rescue Errno::ESRCH
  nil
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

def kill_overdue(pid, label, report, message)
  Process.kill("USR2", pid)
  sleep 2
  stacks = native_stacks(pid)
  kill_group(pid)
  _, status = Process.wait2(pid)
  threads = File.exist?("#{report}.threads") ? File.read("#{report}.threads") : ""
  File.write(File.join(WORK, "#{label.tr("/#", "__")}.timeout.txt"), "#{threads}\n\n#{stacks}")
  [status, { "status" => "timeout", "message" => message }]
rescue Errno::ESRCH
  _, status = Process.wait2(pid)
  [status, { "status" => "timeout", "message" => message }]
end

def check_guard(report)
  marker = "#{report}.guard"
  abort "upstream harness: the guard tripped in a test process: #{File.read(marker).strip}" if File.exist?(marker)
end

# One process runs the files, as upstream's Rake task does. A test that hangs,
# crashes or exits is recorded and the run resumes without the tests already
# finished. A death or hang outside any test is a suite failure.
def run_suite(options, ledger)
  files = options[:files] || (Dir.chdir(UPSTREAM) { Dir["test/*_test.rb"] } + ["test/test_object_identity_fix.rb"]).sort
  results = {}
  suite = []
  attempt = 0
  log = nil
  loop do
    attempt += 1
    before = results.size
    stem = File.join(WORK, "#{options[:mode]}-#{LEG}-#{options[:seed]}-#{attempt}")
    report = "#{stem}.events.jsonl"
    log = "#{stem}.log"
    FileUtils.rm_f([report, "#{report}.threads", "#{report}.guard"])
    pid = spawn_clean(child_env(options[:mode], report), *suite_command(files, options, results.keys),
                      chdir: UPSTREAM, out: log, err: %i[child out],)

    started = {}
    offset = 0
    status = nil
    overdue = nil
    quiet_since = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    until status
      _, status = Process.wait2(pid, Process::WNOHANG)
      events, offset = read_events(report, offset)
      now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      quiet_since = now unless events.empty?
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
      if id
        limit = UpstreamLedger.timeout_for(ledger[id], LEG)
        if now - at > limit
          status, results[id] = kill_overdue(pid, id, report, "exceeded #{limit}s")
          started.delete(id)
          overdue = id
        end
      elsif now - quiet_since > SUITE_IDLE_LIMIT
        status, = kill_overdue(pid, "suite-attempt-#{attempt}", report, "")
        suite << "attempt #{attempt}: no test ran for #{SUITE_IDLE_LIMIT}s and the process did not exit"
      end
      sleep 0.2
    end

    check_guard(report)
    abort "upstream harness: the guard tripped; see #{log}" if status.exitstatus == 97

    reason = status.signaled? ? "signal #{status.termsig}" : "exit #{status.exitstatus}"
    started.each_key { |id| results[id] = { "status" => "crash", "message" => reason } }
    clean = status.exited? && CLEAN_EXITS.include?(status.exitstatus)
    break if clean && started.empty?
    # The watchdog killed a test in flight: resume after it.
    next if overdue

    if started.empty?
      suite << "attempt #{attempt}: the suite process ended with #{reason} outside any test" unless suite.any?
      break
    end
    next unless results.size == before

    warn File.read(log).lines.last(40).join
    abort "upstream harness: the suite process made no progress (#{status.inspect}); see #{log}"
  end
  if results.empty?
    warn File.read(log).lines.last(40).join if log && File.exist?(log)
    abort "upstream harness: no test reported a result (filter #{options[:filter].inspect})"
  end
  [results, suite]
end

def wait_with_limit(pid, limit)
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  loop do
    _, status = Process.wait2(pid, Process::WNOHANG)
    return status if status

    kill_group(pid) if Process.clock_gettime(Process::CLOCK_MONOTONIC) - started > limit
    sleep 0.2
  end
end

def run_scripts(options)
  entries = Tomlrb.load_file(options[:scripts_ledger]).fetch("scripts")
  done = Tomlrb.load_file(UpstreamLedger::MILESTONES).fetch("milestones").select { |_, m| m["done"] }.keys
  entries.filter_map do |script, entry|
    next if entry["weekly"] && !options[:weekly]

    log = File.join(WORK, "#{File.basename(script, ".rb")}-#{LEG}.log")
    report = File.join(WORK, "scripts.events.jsonl")
    FileUtils.rm_f("#{report}.guard")
    pid = spawn_clean(child_env("candidate", report), RbConfig.ruby, script, chdir: UPSTREAM, out: log,
                                                                             err: %i[child out],)
    status = wait_with_limit(pid, entry.fetch("timeout_s"))
    check_guard(report)
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
    entry = UpstreamLedger.effective(ledger[id], LEG)
    next problems << "#{id}: not in the ledger (#{result["status"]})" unless entry

    verdict = UpstreamLedger.verdict(entry["status"], result["status"])
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
results, suite = run_suite(options, ledger)

out = options[:out] || File.join(WORK, "#{options[:mode]}-#{LEG}-#{options[:seed]}.jsonl")
meta = { "mode" => options[:mode], "leg" => LEG, "seed" => options[:seed] }
lines = results.sort.map { |id, result| JSON.generate(result.merge("id" => id).merge(meta)) }
lines += suite.map { |message| JSON.generate({ "id" => UpstreamLedger::SUITE_ID, "status" => "crash", "message" => message }.merge(meta)) }
File.write(out, "#{lines.join("\n")}\n")

counts = results.values.map { |result| result["status"] }.tally.sort.map { |status, n| "#{n} #{status}" }
puts "#{options[:mode]} #{LEG} seed #{options[:seed]}: #{results.size} tests, #{counts.join(", ")}"
suite.each { |message| puts "suite failure: #{message}" }
puts "results: #{out}"
exit unless options[:strict]

problems = suite.map { |message| "suite: #{message}" } + ledger_problems(ledger, results, options)
if problems.empty?
  puts "ledger: every result matches #{File.basename(options[:ledger])} for #{LEG}"
else
  puts problems.sort
  abort "ledger: #{problems.size} mismatch(es) for #{LEG}"
end
