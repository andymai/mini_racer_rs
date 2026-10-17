# frozen_string_literal: true

# Informational comparison of mini_racer_rs with the real mini_racer 0.22.1:
# each implementation runs bench/measure.rb in its own process, alternating, and
# the medians across runs are compared. The reference must support every
# metric; a candidate metric it cannot run yet is reported, not failed.

require "fileutils"
require "json"
require "open3"
require "optparse"
require_relative "../test/consumers/workloads"
require_relative "../test/diff/workers"

ROOT = File.expand_path("..", __dir__)
ROLES = %w[reference candidate].freeze
def number(value) = value.is_a?(Numeric) ? value : nil
def scaled(by, *path) = ->(r) { number(r.dig(*path))&./(by) }
def micros(key, at) = scaled(1e3, key, at)
def millis(key, at) = scaled(1e6, key, at)

METRICS = {
  "eval round trip p50 (us)" => [micros("eval_rtt_ns", "p50"), :lower],
  "eval round trip p99 (us)" => [micros("eval_rtt_ns", "p99"), :lower],
  "eval round trip p99.9 (us)" => [micros("eval_rtt_ns", "p999"), :lower],
  "call round trip p50 (us)" => [micros("call_rtt_ns", "p50"), :lower],
  "call round trip p99 (us)" => [micros("call_rtt_ns", "p99"), :lower],
  "call round trip p99.9 (us)" => [micros("call_rtt_ns", "p999"), :lower],
  "call with timeout p50 (us)" => [micros("call_timeout_rtt_ns", "p50"), :lower],
  "call with timeout p99 (us)" => [micros("call_timeout_rtt_ns", "p99"), :lower],
  "call with timeout p99.9 (us)" => [micros("call_timeout_rtt_ns", "p999"), :lower],
  "callback round trip p50 (us)" => [micros("callback_rtt_ns", "p50"), :lower],
  "startup p50 (ms)" => [millis("startup_ns", "p50"), :lower],
  "startup and load Babel p50 (ms)" => [millis("babel_eval_startup_ns", "p50"), :lower],
  "startup from a Babel snapshot p50 (ms)" => [millis("babel_snapshot_startup_ns", "p50"), :lower],
  "Babel snapshot size (MB)" => [scaled(1e6, "babel_snapshot_bytes"), :lower],
  "JS to Ruby (MB/s)" => [->(r) { number(r["js_to_ruby_mbps"]) }, :higher],
  "Ruby to JS (MB/s)" => [->(r) { number(r["ruby_to_js_mbps"]) }, :higher],
  "idle CPU, 50 contexts (% of a core)" => [->(r) { number(r["idle_cpu_percent"]) }, :lower, 1.0],
}.freeze
WORKLOADS = {
  "babel_transform" => ["Babel transform of a 3 KB JSX component, through ExecJS (ms)", 1e6],
  "terser_minify" => ["Terser minify of a 78 KB library, through ExecJS (ms)", 1e6],
  "react_render" => ["React server render of 1,000 items (17 KB of HTML), through ExecJS (us)", 1e3],
  "react_first_render" => ["React in a new context: load the bundle and render 1,000 items, through ExecJS (ms)", 1e6],
  "handlebars_render" => ["Handlebars render of 50 rows, through Context#call (us)", 1e3],
}.freeze
TERSER_INPUT = File.join(ROOT, "test", "consumers", "js", "node_modules", "handlebars", "dist", "handlebars.runtime.js")
# More spread than this between runs means something else was using the machine.
NOISE = 0.10

options = { runs: 5, quick: false, cpus: nil, serde: true, micro: true, workloads: true }
OptionParser.new do |opts|
  opts.on("--runs N", Integer) { |n| options[:runs] = n }
  opts.on("--quick", "One short run each, for a smoke test") { options.merge!(quick: true, runs: 1) }
  opts.on("--cpus LIST", "Pin both implementations to these CPUs with taskset") { |list| options[:cpus] = list }
  opts.on("--[no-]serde", "Also run mini_racer's own benchmark/serde suite (default on)") { |on| options[:serde] = on }
  opts.on("--[no-]micro", "Run the round-trip, startup and payload metrics (default on)") { |on| options[:micro] = on }
  opts.on("--[no-]workloads", "Run Babel, Terser, React and Handlebars workloads (default on)") do |on|
    options[:workloads] = on
  end
end.parse!
if options[:workloads]
  begin
    DiffConsumers.cases
  rescue RuntimeError => e
    abort "bench: #{e.message}"
  end
  abort "bench: #{TERSER_INPUT} is missing; run `npm ci` in test/consumers/js" unless File.exist?(TERSER_INPUT)
end

# The serde suite reads these too, and an exported one would change its
# workload without a word in the table.
SUITE_ENV = %w[BENCH_SCALE BENCH_WARMUP BENCH_ROUNDS BENCH_SINGLE_THREADED].to_h { |name| [name, nil] }.freeze

def launch(role, options, script, *)
  pin = options[:cpus] ? ["taskset", "-c", options[:cpus]] : []
  command = [*pin, *DiffWorkers.ruby(role, File.join(__dir__, script), role, *)]
  env = DiffWorkers.env(role).merge(SUITE_ENV)
  out, err, status = DiffWorkers.unbundled { Open3.capture3(env, *command, chdir: ROOT) }
  return JSON.parse(out) if status.success?

  abort "bench: the #{role} run of #{script} failed:\n#{err}" if role == "reference"

  warn "bench: the candidate run of #{script} failed:\n#{err.lines.last(5).join}"
  nil
end

def measure(role, options)
  launch(role, options, "measure.rb", *(options[:quick] ? ["--quick"] : [])) || {}
end

def serde(role, options)
  size = options[:quick] ? %w[--rounds 1 --scale 0.1] : %w[--rounds 3]
  result = launch(role, options, "serde.rb", "--json", *size)
  return {} unless result

  result.fetch("benchmarks").to_h { |bench| [bench.fetch("name"), bench.fetch("ms_per_iter") * 1e3] }
end

def workloads(role, options)
  WORKLOADS.keys.to_h do |key|
    [key, launch(role, options, "workloads.rb", key, *(options[:quick] ? ["--quick"] : [])) || {}]
  end
end

def median(values) = values.sort[values.size / 2]

raw = Hash.new { |hash, key| hash[key] = [] }
serde_raw = Hash.new { |hash, key| hash[key] = [] }
workloads_raw = Hash.new { |hash, key| hash[key] = [] }
options[:runs].times do |run|
  ROLES.each do |role|
    raw[role] << measure(role, options) if options[:micro]
    serde_raw[role] << serde(role, options) if options[:serde]
    workloads_raw[role] << workloads(role, options) if options[:workloads]
    warn "run #{run + 1}/#{options[:runs]} #{role} done"
  end
end

broken = METRICS.filter_map { |name, (read, _)| name if raw["reference"].any? { |result| read.call(result).nil? } }
abort "bench: the real mini_racer could not run #{broken.join(", ")}; see tmp/bench" unless broken.empty?
reference_gem, candidate_gem = ROLES.zip(%w[mini_racer mini_racer_rs]).map do |role, plain|
  [raw[role].first, *workloads_raw[role].first&.values].filter_map { |result| result&.fetch("gem", nil) }.first || plain
end
engines = ROLES.map { |role| workloads_raw[role].first&.values&.filter_map { |result| result["engine"] }&.first }

digests = ROLES.to_h { |role| [role, raw[role].filter_map { |result| result["payload_digest"] }.uniq] }
mismatch = !digests["candidate"].empty? && digests["candidate"] != digests["reference"]

governor = begin
  File.read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").strip
rescue Errno::ENOENT
  "unknown"
end
puts "#{reference_gem} against #{candidate_gem}, Ruby #{RUBY_VERSION}, " \
     "#{options[:runs]} run(s), medians, governor #{governor}, " \
     "#{options[:cpus] ? "CPUs #{options[:cpus]}" : "not pinned"}"
puts "Engines: #{engines.join(" against ")}" if engines.all?
# Ratios of near-zero values say nothing, so with either value below the
# floor none is given.
def row(name, values, direction, floor: nil, wrong: false)
  medians = values.transform_values { |list| list.compact.size == list.size ? median(list) : nil }
  reference, candidate = medians.values_at(*ROLES)
  below = floor && candidate && [reference, candidate].min < floor
  ratio = (direction == :lower ? reference / candidate : candidate / reference) if candidate && !wrong && !below
  noisy = values.values.any? { |list| list.compact.size > 1 && (list.compact.max / list.compact.min) - 1 > NOISE }
  note = if wrong then "different output"
         elsif candidate.nil? then "unsupported"
         elsif below then "under #{floor}"
         elsif noisy then "noisy"
         else ""
         end
  cells = [reference, candidate].map { |value| value ? format("%.2f", value) : "n/a" }
  puts "| #{name} | #{cells.join(" | ")} | #{ratio ? format("%.2fx", ratio) : "n/a"} | #{note} |"
  { "reference" => reference, "candidate" => candidate, "ratio" => ratio, "note" => note }
end

summary = {}
if options[:micro]
  puts
  puts "| Metric | mini_racer | mini_racer_rs | Ratio | Note |"
  puts "|---|---|---|---|---|"
  summary = METRICS.to_h do |name, (read, direction, floor)|
    values = ROLES.to_h { |role| [role, raw[role].map(&read)] }
    [name, row(name, values, direction, floor:, wrong: mismatch && name.start_with?("JS to Ruby"))]
  end
end

workloads_summary = {}
if options[:workloads]
  puts
  puts "Workloads, medians of each run's median."
  puts
  puts "| Workload | mini_racer | mini_racer_rs | Ratio | Note |"
  puts "|---|---|---|---|---|"
  workloads_summary = WORKLOADS.to_h do |key, (label, unit)|
    values = ROLES.to_h do |role|
      [role, workloads_raw[role].map { |run| number(run.dig(key, "result", "ns"))&./(unit) }]
    end
    reference, candidate = ROLES.map do |role|
      workloads_raw[role].filter_map { |run| run.dig(key, "result", "digest") }.uniq
    end
    [label, row(label, values, :lower, wrong: !candidate.empty? && candidate != reference)]
  end
end

serde_summary = {}
unless serde_raw.empty?
  puts
  puts "mini_racer's benchmark/serde, microseconds per iteration, medians. Its context has a 120 s timeout, " \
       "so every case pays for arming it."
  puts
  puts "| Case | mini_racer | mini_racer_rs | Ratio | Note |"
  puts "|---|---|---|---|---|"
  serde_summary = serde_raw["reference"].first.keys.to_h do |name|
    [name, row(name, ROLES.to_h { |role| [role, serde_raw[role].map { |run| run[name] }] }, :lower)]
  end
end

out = File.join(ROOT, "tmp", "bench", "#{Time.now.utc.strftime("%Y%m%dT%H%M%SZ")}.json")
FileUtils.mkdir_p(File.dirname(out))
File.write(out, JSON.pretty_generate({ "governor" => governor, "cpus" => options[:cpus], "runs" => options[:runs],
                                       "summary" => summary, "serde" => serde_summary,
                                       "workloads" => workloads_summary, "raw" => raw, "serde_raw" => serde_raw,
                                       "workloads_raw" => workloads_raw, }),)
puts
puts "results: #{out.delete_prefix("#{ROOT}/")}"
