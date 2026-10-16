# frozen_string_literal: true

# Differential run: the same cases go to the real mini_racer 0.22.1 and to
# mini_racer_rs, and every outcome must match, match an accepted divergence, or
# be a known gap in the ledger. --twin also runs a second copy of the real gem,
# which must agree with the first, or the harness itself is unsound.

require "fileutils"
require "json"
require "optparse"
require "tomlrb"
require_relative "generate"
require_relative "ledger"
require_relative "shrink"
require_relative "../upstream/ledger"

ROOT = File.expand_path("../..", __dir__)
LEDGER = File.join(__dir__, "candidate.status.toml")
DIVERGENCES = File.join(__dir__, "divergences.toml")
CORPUS = File.join(__dir__, "corpus")
WORK = File.join(ROOT, "tmp", "diff")

class HarnessError < StandardError; end

options = { seed: 1, js: 200, ruby: 100, twin: false, strict: false, write_ledger: false, shrink: false }
OptionParser.new do |opts|
  opts.banner = "usage: run.rb [options]"
  opts.on("--seed N", Integer) { |seed| options[:seed] = seed }
  opts.on("--js N", Integer, "Generated JS value cases") { |n| options[:js] = n }
  opts.on("--ruby N", Integer, "Generated Ruby argument cases") { |n| options[:ruby] = n }
  opts.on("--twin", "Also run a second copy of the real gem, which must match the first") { options[:twin] = true }
  opts.on("--strict", "Fail unless every outcome matches test/diff/candidate.status.toml") { options[:strict] = true }
  opts.on("--write-ledger", "Record this run's outcomes in test/diff/candidate.status.toml") do
    options[:write_ledger] = true
  end
  opts.on("--shrink", "Minimize each new divergence into test/diff/corpus") { options[:shrink] = true }
  opts.on("--out PATH") { |path| options[:out] = path }
end.parse!

# A long-lived process for one side. It must announce the gem it loaded before
# any case runs; a crash or hang is the outcome of the step in flight, and the
# next case gets a fresh process.
class Worker
  CASE_LIMIT = 30
  START_LIMIT = 120
  HARNESS_ERROR = 3

  attr_reader :gem

  def initialize(role, label = role)
    @role = role
    @log = File.join(WORK, "#{label}-worker.log")
    start
  end

  def env
    common = { "TZ" => "UTC" }
    if @role == "candidate"
      # with_unbundled_env drops every BUNDLE_* variable, and CI installs some
      # groups only; the child must ask Bundler for the same groups.
      return common.merge("BUNDLE_GEMFILE" => File.join(ROOT, "Gemfile"),
                          "BUNDLE_WITHOUT" => ENV.fetch("BUNDLE_WITHOUT", nil),)
    end

    common.merge("BUNDLE_GEMFILE" => File.join(ROOT, "tools/reference/Gemfile"),
                 "BUNDLE_PATH" => File.join(ROOT, "tools/reference/vendor/bundle"),)
  end

  def start
    args = ["bundle", "exec", "ruby", *(@role == "candidate" ? ["-I#{File.join(ROOT, "lib")}"] : []),
            File.join(__dir__, "worker.rb"), @role,]
    spawn = lambda do
      IO.popen(env, args, "r+", chdir: ROOT, pgroup: true, err: [@log, "a"], external_encoding: Encoding::UTF_8)
    end
    @io = defined?(Bundler) ? Bundler.with_unbundled_env(&spawn) : spawn.call
    line = @io.gets if @io.wait_readable(START_LIMIT)
    ready = line && JSON.parse(line)
    raise HarnessError, "the #{@role} worker did not start; see #{@log}" unless ready&.fetch("ready", nil) == @role

    @gem = ready.fetch("gem")
  end

  def submit(kase)
    @io.puts(JSON.generate(kase))
    @io.flush
  rescue Errno::EPIPE, IOError
    nil
  end

  def collect
    outcomes = []
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + CASE_LIMIT
    loop do
      remaining = deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
      return outcomes << timed_out unless remaining.positive? && @io.wait_readable(remaining)

      line = @io.gets
      return outcomes << died unless line

      message = JSON.parse(line)
      return outcomes if message.key?("done")

      outcomes << message.fetch("outcome")
    end
  end

  def died
    @io.close
    status = Process.last_status
    raise HarnessError, "the #{@role} worker could not decode a case; see #{@log}" if status.exitstatus == HARNESS_ERROR

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

# Every side runs a case at the same time; the outcomes are read in turn.
def run_case(workers, kase)
  workers.each { |worker| worker.submit(kase) }
  workers.map(&:collect)
end

def load_ledger
  File.exist?(LEDGER) ? Tomlrb.load_file(LEDGER).fetch("cases", {}) : {}
end

def load_corpus
  Dir[File.join(CORPUS, "*.json")].flat_map { |path| JSON.parse(File.read(path)) }
end

def compat_entries
  path = File.join(ROOT, "compat.toml")
  File.exist?(path) ? Tomlrb.load_file(path).fetch("ids", {}) : {}
end

def shrink(workers, kase, outcomes, divergences)
  target = DiffLedger.signature(*outcomes)
  DiffShrink.minimize(kase) do |variant|
    expected, actual = run_case(workers, variant)
    DiffLedger.classify(expected, actual, divergences) == "diverged" && DiffLedger.signature(expected, actual) == target
  end
end

FileUtils.mkdir_p(WORK)
divergences = DiffLedger.load_divergences(DIVERGENCES)
problems = DiffLedger.divergence_problems(divergences, compat_entries)
abort problems.join("\n") unless problems.empty?

cases = DiffGenerate.cases(seed: options[:seed], javascript: options[:js], ruby: options[:ruby]) + load_corpus
duplicates = cases.map { |kase| kase.fetch("id") }.tally.select { |_, n| n > 1 }.keys
abort "duplicate case ids: #{duplicates.join(", ")}" unless duplicates.empty?

results = []
twin_mismatches = []
begin
  reference = Worker.new("reference")
  candidate = Worker.new("candidate")
  twin = Worker.new("reference", "reference-twin") if options[:twin]
  puts "reference: #{reference.gem}; candidate: #{candidate.gem}"
  cases.each do |kase|
    expected, actual, again = run_case([reference, candidate, twin].compact, kase)
    twin_mismatches << kase["id"] if twin && again != expected
    results << { "id" => kase["id"], "class" => DiffLedger.classify(expected, actual, divergences),
                 "expected" => expected, "actual" => actual, }
  end
rescue HarnessError => e
  abort "diff: #{e.message}"
end

out = options[:out] || File.join(WORK, "candidate-#{options[:seed]}.jsonl")
File.write(out, results.map { |result| "#{JSON.generate(result)}\n" }.join)
counts = results.map { |result| result["class"].split(":").first }.tally.sort.map { |kind, n| "#{n} #{kind}" }
puts "seed #{options[:seed]}: #{results.size} cases, #{counts.join(", ")}"
puts "results: #{out}"

if options[:shrink]
  known = load_ledger
  by_id = cases.to_h { |kase| [kase["id"], kase] }
  results.each do |result|
    next unless result["class"] == "diverged" && !known.dig(result["id"], "status").to_s.start_with?("xfail:", "crash:")

    minimal = shrink([reference, candidate], by_id.fetch(result["id"]), [result["expected"], result["actual"]],
                     divergences,)
    minimal = minimal.merge("id" => "min-#{result["id"]}")
    File.write(File.join(CORPUS, "#{minimal["id"]}.json"), "#{JSON.pretty_generate([minimal])}\n")
    puts "minimized #{result["id"]} into test/diff/corpus/#{minimal["id"]}.json"
  end
end
[reference, candidate, twin].compact.each(&:stop)

abort "diff: the two copies of the real gem disagree on #{twin_mismatches.join(", ")}" unless twin_mismatches.empty?
puts "diff: both copies of the real gem agree on every case" if twin

if options[:write_ledger]
  previous = load_ledger
  statuses = results.to_h do |result|
    [result["id"], DiffLedger.ledger_status(previous.dig(result["id"], "status"), result["class"])]
  end
  File.write(LEDGER, DiffLedger.dump_ledger(statuses))
  puts "#{LEDGER}: #{statuses.size} cases"
end
exit unless options[:strict]

problems = DiffLedger.ledger_problems(load_ledger, results, UpstreamLedger.milestones)
unless problems.empty?
  abort "#{problems.sort.join("\n")}\ndiff: #{problems.size} mismatch(es) with #{File.basename(LEDGER)}"
end

puts "diff: every outcome matches #{File.basename(LEDGER)}"
