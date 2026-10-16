# frozen_string_literal: true

# Differential run: the same cases go to the real mini_racer 0.22.1 and to
# mini_racer_rs (or, with --pair reference, to two copies of the real gem,
# which proves the harness itself is deterministic), and every outcome must
# match, match an accepted divergence, or be a known gap in the ledger.

require "fileutils"
require "json"
require "optparse"
require "tomlrb"
require_relative "canonical"
require_relative "generate"
require_relative "ledger"
require_relative "shrink"
require_relative "../upstream/ledger"

ROOT = File.expand_path("../..", __dir__)
LEDGER = File.join(__dir__, "candidate.status.toml")
DIVERGENCES = File.join(__dir__, "divergences.toml")
CORPUS = File.join(__dir__, "corpus")

options = { seed: 1, js: 200, ruby: 100, pair: "candidate", strict: false, write_ledger: false, shrink: false }
OptionParser.new do |opts|
  opts.banner = "usage: run.rb [options]"
  opts.on("--seed N", Integer) { |seed| options[:seed] = seed }
  opts.on("--js N", Integer, "Generated JS value cases") { |n| options[:js] = n }
  opts.on("--ruby N", Integer, "Generated Ruby argument cases") { |n| options[:ruby] = n }
  opts.on("--pair ROLE", %w[candidate reference], "Compare the reference with this") { |role| options[:pair] = role }
  opts.on("--strict", "Fail unless every outcome matches test/diff/candidate.status.toml") { options[:strict] = true }
  opts.on("--write-ledger", "Record this run's outcomes in test/diff/candidate.status.toml") do
    options[:write_ledger] = true
  end
  opts.on("--shrink", "Minimize each new divergence into test/diff/corpus") { options[:shrink] = true }
  opts.on("--out PATH") { |path| options[:out] = path }
end.parse!

# A long-lived process for one side; a crash or hang is that case's outcome,
# and the next case gets a fresh process.
class Worker
  CASE_LIMIT = 30

  def initialize(role)
    @role = role
    start
  end

  def env
    common = { "TZ" => "UTC" }
    return common.merge("BUNDLE_GEMFILE" => File.join(ROOT, "Gemfile")) if @role == "candidate"

    common.merge("BUNDLE_GEMFILE" => File.join(ROOT, "tools/reference/Gemfile"),
                 "BUNDLE_PATH" => File.join(ROOT, "tools/reference/vendor/bundle"),)
  end

  def start
    args = ["bundle", "exec", "ruby", *(@role == "candidate" ? ["-I#{File.join(ROOT, "lib")}"] : []),
            File.join(__dir__, "worker.rb"), @role,]
    log = File.join(ROOT, "tmp", "diff", "#{@role}-worker.log")
    FileUtils.mkdir_p(File.dirname(log))
    spawn = -> { IO.popen(env, args, "r+", chdir: ROOT, pgroup: true, err: [log, "a"]) }
    @io = defined?(Bundler) ? Bundler.with_unbundled_env(&spawn) : spawn.call
  end

  # A process that dies reports how, which is the same on both sides when both
  # gems crash on a case (mini_racer 0.22.1 aborts on some Map keys).
  def run(kase)
    @io.puts(JSON.generate(kase))
    @io.flush
    ready = @io.wait_readable(CASE_LIMIT)
    line = ready && @io.gets
    return JSON.parse(line).fetch("outcomes") if line

    [ready ? died : timed_out]
  rescue Errno::EPIPE, IOError
    [died]
  end

  def died
    @io.close
    status = Process.last_status
    start
    ["crash", status.signaled? ? "signal #{status.termsig}" : "exit #{status.exitstatus}"]
  end

  def timed_out
    stop
    start
    ["timeout", CASE_LIMIT]
  end

  def stop
    Process.kill("KILL", -@io.pid)
  rescue Errno::ESRCH
    nil
  ensure
    @io.close unless @io.closed?
  end
end

def load_corpus
  Dir[File.join(CORPUS, "*.json")].flat_map { |path| JSON.parse(File.read(path)) }
end

divergences = DiffLedger.load_divergences(DIVERGENCES)
compat_path = File.join(ROOT, "compat.toml")
compat = File.exist?(compat_path) ? Tomlrb.load_file(compat_path).fetch("ids", {}) : {}
problems = DiffLedger.divergence_problems(divergences, compat)
abort problems.join("\n") unless problems.empty?

cases = DiffGenerate.cases(seed: options[:seed], javascript: options[:js], ruby: options[:ruby]) + load_corpus
duplicates = cases.map { |kase| kase.fetch("id") }.tally.select { |_, n| n > 1 }.keys
abort "duplicate case ids: #{duplicates.join(", ")}" unless duplicates.empty?

reference = Worker.new("reference")
other = Worker.new(options[:pair])
results = cases.map do |kase|
  expected = reference.run(kase)
  actual = other.run(kase)
  { "id" => kase["id"], "class" => DiffLedger.classify(expected, actual, divergences), "expected" => expected,
    "actual" => actual, }
end

out = options[:out] || File.join(ROOT, "tmp", "diff", "#{options[:pair]}-#{options[:seed]}.jsonl")
FileUtils.mkdir_p(File.dirname(out))
File.write(out, results.map { |result| "#{JSON.generate(result)}\n" }.join)
counts = results.map { |result| result["class"].split(":").first }.tally.sort.map { |kind, n| "#{n} #{kind}" }
puts "reference vs #{options[:pair]}, seed #{options[:seed]}: #{results.size} cases, #{counts.join(", ")}"
puts "results: #{out}"

if options[:shrink]
  new_divergences = results.select { |result| result["class"] == "diverged" }
  previous = File.exist?(LEDGER) ? UpstreamLedger.load(LEDGER) : {}
  new_divergences.reject! { |result| previous.dig(result["id"], "status").to_s.start_with?("xfail:") }
  by_id = cases.to_h { |kase| [kase["id"], kase] }
  new_divergences.each do |result|
    minimal = DiffShrink.minimize(by_id.fetch(result["id"])) do |variant|
      DiffLedger.classify(reference.run(variant), other.run(variant), divergences) == "diverged"
    end
    minimal = minimal.merge("id" => "min-#{result["id"]}")
    File.write(File.join(CORPUS, "#{minimal["id"]}.json"), "#{JSON.pretty_generate([minimal])}\n")
    puts "minimized #{result["id"]} into test/diff/corpus/#{minimal["id"]}.json"
  end
end
reference.stop
other.stop

if options[:pair] == "reference"
  diverged = results.reject { |result| result["class"] == "same" }
  abort "reference vs reference: #{diverged.size} divergence(s); see #{out}" unless diverged.empty?

  puts "reference vs reference: every outcome matches"
  exit
end

if options[:write_ledger]
  previous = File.exist?(LEDGER) ? Tomlrb.load_file(LEDGER).fetch("cases", {}) : {}
  statuses = results.to_h do |result|
    [result["id"], DiffLedger.ledger_status(previous.dig(result["id"], "status"), result["class"])]
  end
  File.write(LEDGER, DiffLedger.dump_ledger(statuses))
  puts "#{LEDGER}: #{statuses.size} cases"
end
exit unless options[:strict]

ledger = File.exist?(LEDGER) ? Tomlrb.load_file(LEDGER).fetch("cases", {}) : {}
problems = DiffLedger.ledger_problems(ledger, results, UpstreamLedger.milestones)
unless problems.empty?
  abort "#{problems.sort.join("\n")}\ndiff: #{problems.size} mismatch(es) with #{File.basename(LEDGER)}"
end

puts "diff: every outcome matches #{File.basename(LEDGER)}"
