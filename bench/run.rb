# frozen_string_literal: true

# Informational comparison of mini_racer_rs with the real mini_racer 0.22.1:
# each implementation runs bench/measure.rb in its own process, alternating, and
# the medians across runs are compared. The reference must support every
# metric; a candidate metric it cannot run yet is reported, not failed.

require "fileutils"
require "json"
require "open3"
require "optparse"
require_relative "../test/diff/workers"

ROOT = File.expand_path("..", __dir__)
ROLES = %w[reference candidate].freeze
def number(value) = value.is_a?(Numeric) ? value : nil
def micros(key, at) = ->(r) { number(r.dig(key, at))&./(1e3) }
def millis(key, at) = ->(r) { number(r.dig(key, at))&./(1e6) }

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
  "JS to Ruby (MB/s)" => [->(r) { number(r["js_to_ruby_mbps"]) }, :higher],
  "Ruby to JS (MB/s)" => [->(r) { number(r["ruby_to_js_mbps"]) }, :higher],
  "idle CPU, 50 contexts (% of a core)" => [->(r) { number(r["idle_cpu_percent"]) }, :lower, 1.0],
}.freeze
SERDE = File.join(ROOT, "vendor/mini_racer/benchmark/serde/bench.rb")
# More spread than this between runs means something else was using the machine.
NOISE = 0.10

options = { runs: 5, quick: false, cpus: nil, serde: true }
OptionParser.new do |opts|
  opts.on("--runs N", Integer) { |n| options[:runs] = n }
  opts.on("--quick", "One short run each, for a smoke test") { options.merge!(quick: true, runs: 1) }
  opts.on("--cpus LIST", "Pin both implementations to these CPUs with taskset") { |list| options[:cpus] = list }
  opts.on("--[no-]serde", "Also run mini_racer's own benchmark/serde suite (default on)") { |on| options[:serde] = on }
end.parse!

def launch(role, options, script, *)
  pin = options[:cpus] ? ["taskset", "-c", options[:cpus]] : []
  command = [*pin, *DiffWorkers.ruby(role, script, *)]
  out, err, status = DiffWorkers.unbundled { Open3.capture3(DiffWorkers.env(role), *command, chdir: ROOT) }
  abort "bench: the #{role} run of #{File.basename(script)} failed:\n#{err}" unless status.success?

  JSON.parse(out)
end

def measure(role, options)
  launch(role, options, File.join(__dir__, "measure.rb"), role, *(options[:quick] ? ["--quick"] : []))
end

# mini_racer's suite times each case as milliseconds per iteration.
def serde(role, options)
  scale = options[:quick] ? %w[--scale 0.1] : []
  launch(role, options, SERDE, "--json", "--rounds", "3", *scale)
    .fetch("benchmarks").to_h { |bench| [bench.fetch("name"), bench.fetch("ms_per_iter")] }
end

def median(values) = values.sort[values.size / 2]

raw = Hash.new { |hash, key| hash[key] = [] }
serde_raw = Hash.new { |hash, key| hash[key] = [] }
options[:runs].times do |run|
  ROLES.each do |role|
    raw[role] << measure(role, options)
    serde_raw[role] << serde(role, options) if options[:serde]
    warn "run #{run + 1}/#{options[:runs]} #{role} done"
  end
end

broken = METRICS.filter_map { |name, (read, _)| name if raw["reference"].any? { |result| read.call(result).nil? } }
abort "bench: the real mini_racer could not run #{broken.join(", ")}; see tmp/bench" unless broken.empty?

digests = ROLES.to_h { |role| [role, raw[role].filter_map { |result| result["payload_digest"] }.uniq] }
mismatch = !digests["candidate"].empty? && digests["candidate"] != digests["reference"]

governor = begin
  File.read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").strip
rescue Errno::ENOENT
  "unknown"
end
puts "#{raw["reference"].first["gem"]} against #{raw["candidate"].first["gem"]}, Ruby #{RUBY_VERSION}, " \
     "#{options[:runs]} run(s), medians, governor #{governor}, " \
     "#{options[:cpus] ? "CPUs #{options[:cpus]}" : "not pinned"}"
puts
puts "| Metric | mini_racer | mini_racer_rs | Ratio | Note |"
puts "|---|---|---|---|---|"
summary = METRICS.to_h do |name, (read, direction, floor)|
  values = ROLES.to_h { |role| [role, raw[role].map(&read)] }
  medians = values.transform_values { |list| list.compact.size == list.size ? median(list) : nil }
  reference, candidate = medians.values_at(*ROLES)
  wrong = mismatch && name.start_with?("JS to Ruby")
  # Ratios of near-zero values say nothing, so below the floor none is given.
  meaningful = candidate && !wrong && (floor.nil? || [reference, candidate].max >= floor)
  ratio = (direction == :lower ? reference / candidate : candidate / reference) if meaningful
  noisy = values.values.any? { |list| list.compact.size > 1 && (list.compact.max / list.compact.min) - 1 > NOISE }
  note = if wrong then "different output"
         elsif candidate.nil? then "unsupported"
         elsif noisy then "noisy"
         else ""
         end
  cells = [reference, candidate].map { |value| value ? format("%.2f", value) : "n/a" }
  puts "| #{name} | #{cells.join(" | ")} | #{ratio ? format("%.2fx", ratio) : "n/a"} | #{note} |"
  [name, { "reference" => reference, "candidate" => candidate, "ratio" => ratio, "note" => note }]
end

serde_summary = {}
unless serde_raw.empty?
  puts
  puts "mini_racer's benchmark/serde, milliseconds per iteration, medians"
  puts
  puts "| Case | mini_racer | mini_racer_rs | Ratio |"
  puts "|---|---|---|---|"
  serde_raw["reference"].first.each_key do |name|
    reference, candidate = ROLES.map { |role| median(serde_raw[role].map { |run| run.fetch(name) }) }
    serde_summary[name] = { "reference" => reference, "candidate" => candidate, "ratio" => reference / candidate }
    cells = [name, format("%.4f", reference), format("%.4f", candidate), format("%.2fx", reference / candidate)]
    puts "| #{cells.join(" | ")} |"
  end
end

out = File.join(ROOT, "tmp", "bench", "#{Time.now.utc.strftime("%Y%m%dT%H%M%SZ")}.json")
FileUtils.mkdir_p(File.dirname(out))
File.write(out, JSON.pretty_generate({ "governor" => governor, "cpus" => options[:cpus], "runs" => options[:runs],
                                       "summary" => summary, "serde" => serde_summary, "raw" => raw,
                                       "serde_raw" => serde_raw, }),)
puts
puts "results: #{out.delete_prefix("#{ROOT}/")}"
