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
METRICS = {
  "eval round trip p50 (us)" => [->(r) { r.dig("eval_rtt_ns", "p50")&./(1e3) }, :lower],
  "call round trip p50 (us)" => [->(r) { r.dig("call_rtt_ns", "p50")&./(1e3) }, :lower],
  "startup p50 (ms)" => [->(r) { r.dig("startup_ns", "p50")&./(1e6) }, :lower],
  "JS to Ruby (MB/s)" => [->(r) { r["js_to_ruby_mbps"].is_a?(Numeric) ? r["js_to_ruby_mbps"] : nil }, :higher],
  "Ruby to JS (MB/s)" => [->(r) { r["ruby_to_js_mbps"].is_a?(Numeric) ? r["ruby_to_js_mbps"] : nil }, :higher],
}.freeze
# More spread than this between runs means something else was using the machine.
NOISE = 0.10

options = { runs: 5, quick: false, cpus: nil }
OptionParser.new do |opts|
  opts.on("--runs N", Integer) { |n| options[:runs] = n }
  opts.on("--quick", "One short run each, for a smoke test") { options.merge!(quick: true, runs: 1) }
  opts.on("--cpus LIST", "Pin both implementations to these CPUs with taskset") { |list| options[:cpus] = list }
end.parse!

def measure(role, options)
  pin = options[:cpus] ? ["taskset", "-c", options[:cpus]] : []
  args = [*pin, *DiffWorkers.ruby(role, File.join(__dir__, "measure.rb"), role, *(options[:quick] ? ["--quick"] : []))]
  env = DiffWorkers.env(role)
  out, err, status = DiffWorkers.unbundled { Open3.capture3(env, *args, chdir: ROOT) }
  abort "bench: the #{role} run failed:\n#{err}" unless status.success?

  JSON.parse(out)
end

def median(values) = values.sort[values.size / 2]

raw = Hash.new { |hash, key| hash[key] = [] }
options[:runs].times do |run|
  ROLES.each do |role|
    raw[role] << measure(role, options)
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
summary = METRICS.to_h do |name, (read, direction)|
  values = ROLES.to_h { |role| [role, raw[role].map(&read)] }
  medians = values.transform_values { |list| list.compact.size == list.size ? median(list) : nil }
  reference, candidate = medians.values_at(*ROLES)
  wrong = mismatch && name.start_with?("JS to Ruby")
  ratio = (direction == :lower ? reference / candidate : candidate / reference) if candidate && !wrong
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

out = File.join(ROOT, "tmp", "bench", "#{Time.now.utc.strftime("%Y%m%dT%H%M%SZ")}.json")
FileUtils.mkdir_p(File.dirname(out))
File.write(out, JSON.pretty_generate({ "governor" => governor, "cpus" => options[:cpus], "runs" => options[:runs],
                                       "summary" => summary, "raw" => raw, }),)
puts
puts "results: #{out.delete_prefix("#{ROOT}/")}"
