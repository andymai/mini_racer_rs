# frozen_string_literal: true

# Differential run: the same cases go to the real mini_racer 0.22.2 and to
# mini_racer_rs, and every outcome must match, match an accepted divergence, or
# be a known gap in the ledger. --set consumers runs real libraries (Babel,
# Terser, Handlebars, React) instead of generated values, against their own
# ledger. --twin also runs a second copy of the real gem, which must agree with
# the first, or the harness itself is unsound. --explore runs a fresh seed's
# cases with no ledger and fails on any divergence the accepted ones miss. Its
# cases share no object, so it accepts none under behavior:object-ids@ruby: a
# repeated object there is one the candidate made up. With --shared its cases
# share objects as the ledger's do, and those divergences stay accepted.

require "fileutils"
require "json"
require "optparse"
require "tomlrb"
require_relative "generate"
require_relative "ledger"
require_relative "shrink"
require_relative "workers"
require_relative "../consumers/workloads"
require_relative "../upstream/ledger"

ROOT = File.expand_path("../..", __dir__)
LEDGERS = { "values" => File.join(__dir__, "candidate.status.toml"),
            "consumers" => File.expand_path("../consumers/candidate.status.toml", __dir__), }.freeze
DIVERGENCES = File.join(__dir__, "divergences.toml")
CORPUS = File.join(__dir__, "corpus")
FAILED = %w[raise crash timeout].freeze
NEW_OUTCOMES = %w[diverged crashed].freeze
IDENTITY = "behavior:object-ids@ruby"
WORK = DiffWorkers::WORK

options = { set: "values", seed: 1, js: 200, ruby: 100, twin: false, strict: false, write_ledger: false, shrink: false }
OptionParser.new do |opts|
  opts.banner = "usage: run.rb [options]"
  opts.on("--set NAME", LEDGERS.keys, "Cases to run: values (default) or consumers") { |set| options[:set] = set }
  opts.on("--seed N", Integer) { |seed| options.merge!(seed: seed, generated: true) }
  opts.on("--js N", Integer, "Generated JS value cases") { |n| options.merge!(js: n, generated: true) }
  opts.on("--ruby N", Integer, "Generated Ruby argument cases") { |n| options.merge!(ruby: n, generated: true) }
  opts.on("--twin", "Also run a second copy of the real gem, which must match the first") { options[:twin] = true }
  opts.on("--strict", "Fail unless every outcome matches the set's ledger") { options[:strict] = true }
  opts.on("--write-ledger", "Record this run's outcomes in the set's ledger") do
    options[:write_ledger] = true
  end
  opts.on("--shrink", "Minimize each new divergence into test/diff/corpus") do
    options.merge!(shrink: true, generated: true)
  end
  opts.on("--explore", "Generated cases only, none sharing an object; fail on any divergence or crash") do
    options.merge!(explore: true, generated: true)
  end
  opts.on("--shared", "With --explore, cases share objects, and #{IDENTITY} divergences stay accepted") do
    options[:shared] = true
  end
  opts.on("--out PATH") { |path| options[:out] = path }
end.parse!

def load_ledger(path)
  File.exist?(path) ? Tomlrb.load_file(path).fetch("cases", {}) : {}
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
    expected, actual = DiffWorkers.run_case(workers, variant)
    DiffLedger.classify(expected, actual, divergences) == "diverged" && DiffLedger.signature(expected, actual) == target
  end
end

FileUtils.mkdir_p(WORK)
divergences = DiffLedger.load_divergences(DIVERGENCES)
problems = DiffLedger.divergence_problems(divergences, compat_entries)
abort problems.join("\n") unless problems.empty?
shared = !options[:explore] || options.fetch(:shared, false)
divergences = divergences.reject { |divergence| divergence["compat"] == IDENTITY } unless shared

ledger_path = LEDGERS.fetch(options[:set])
if options[:explore] && (options[:strict] || options[:write_ledger])
  abort "diff: --explore checks no ledger, so it takes neither --strict nor --write-ledger"
end
abort "diff: --shared applies to --explore" if options[:shared] && !options[:explore]
if options[:set] == "consumers" && options[:generated]
  abort "diff: --set consumers runs fixed workloads; --seed, --js, --ruby and --shrink apply to the values set"
end
cases = if options[:set] == "consumers"
          begin
            DiffConsumers.cases
          rescue RuntimeError => e
            abort "diff: #{e.message}"
          end
        else
          generated = DiffGenerate.cases(seed: options[:seed], javascript: options[:js], ruby: options[:ruby],
                                         shared:,)
          options[:explore] ? generated : generated + load_corpus
        end
duplicates = cases.map { |kase| kase.fetch("id") }.tally.select { |_, n| n > 1 }.keys
abort "duplicate case ids: #{duplicates.join(", ")}" unless duplicates.empty?

results = []
twin_mismatches = []
begin
  reference = DiffWorkers::Worker.new("reference")
  candidate = DiffWorkers::Worker.new("candidate")
  twin = DiffWorkers::Worker.new("reference", "reference-twin") if options[:twin]
  puts "reference: #{reference.gem}; candidate: #{candidate.gem}"
  cases.each do |kase|
    expected, actual, again = DiffWorkers.run_case([reference, candidate, twin].compact, kase)
    twin_mismatches << kase["id"] if twin && again != expected
    results << { "id" => kase["id"], "class" => DiffLedger.classify(expected, actual, divergences),
                 "expected" => expected, "actual" => actual, }
  end
rescue DiffWorkers::HarnessError => e
  abort "diff: #{e.message}"
end

# A consumer workload is a real library doing real work: if the real gem
# raises or crashes on a step, the workload or its bundle is broken, and a
# matching failure on both sides would only hide that.
if options[:set] == "consumers"
  failed = results.filter_map do |result|
    step = result["expected"].index { |outcome| FAILED.include?(outcome.first) }
    "#{result["id"]} step #{step}: #{JSON.generate(result["expected"][step])[0, 200]}" if step
  end
  abort "diff: the real mini_racer failed consumer workloads:\n#{failed.join("\n")}" unless failed.empty?
end

out = options[:out] || File.join(WORK,
                                 options[:set] == "consumers" ? "consumers.jsonl" : "values-#{options[:seed]}.jsonl",)
File.write(out, results.map { |result| "#{JSON.generate(result)}\n" }.join)
counts = results.map { |result| result["class"].split(":").first }.tally.sort.map { |kind, n| "#{n} #{kind}" }
label = options[:set] == "consumers" ? "consumers" : "values, seed #{options[:seed]}"
puts "#{label}: #{results.size} cases, #{counts.join(", ")}"
puts "results: #{out}"

if options[:shrink]
  known = load_ledger(ledger_path)
  by_id = cases.to_h { |kase| [kase["id"], kase] }
  begin
    results.each do |result|
      next unless result["class"] == "diverged" && !known.dig(result["id"], "status").to_s.start_with?("xfail:",
                                                                                                       "crash:",)

      minimal = shrink([reference, candidate], by_id.fetch(result["id"]), [result["expected"], result["actual"]],
                       divergences,)
      minimal = minimal.merge("id" => "min-#{result["id"]}")
      File.write(File.join(CORPUS, "#{minimal["id"]}.json"), "#{JSON.pretty_generate([minimal])}\n")
      puts "minimized #{result["id"]} into test/diff/corpus/#{minimal["id"]}.json"
    end
  rescue DiffWorkers::HarnessError => e
    abort "diff: #{e.message}"
  end
end
[reference, candidate, twin].compact.each(&:stop)

abort "diff: the two copies of the real gem disagree on #{twin_mismatches.join(", ")}" unless twin_mismatches.empty?
puts "diff: both copies of the real gem agree on every case" if twin

if options[:explore]
  found = results.select { |result| NEW_OUTCOMES.include?(result["class"]) }
  abort "diff: #{found.size} new outcome(s): #{found.map { |result| result["id"] }.join(", ")}" unless found.empty?
  puts "diff: no divergence outside the accepted ones"
end

if options[:write_ledger]
  previous = load_ledger(ledger_path)
  statuses = results.to_h do |result|
    [result["id"], DiffLedger.ledger_status(previous.dig(result["id"], "status"), result["class"])]
  end
  command = "ruby test/diff/run.rb#{" --set consumers" if options[:set] == "consumers"} --write-ledger"
  File.write(ledger_path, DiffLedger.dump_ledger(statuses, command))
  puts "#{ledger_path}: #{statuses.size} cases"
end
exit unless options[:strict]

problems = DiffLedger.ledger_problems(load_ledger(ledger_path), results, UpstreamLedger.milestones)
ledger_name = ledger_path.delete_prefix("#{ROOT}/")
abort "#{problems.sort.join("\n")}\ndiff: #{problems.size} mismatch(es) with #{ledger_name}" unless problems.empty?

puts "diff: every outcome matches #{ledger_name}"
