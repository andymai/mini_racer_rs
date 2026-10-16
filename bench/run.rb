# frozen_string_literal: true

# Informational comparison of mini_racer_rs with the real mini_racer 0.22.1:
# each implementation runs bench/measure.rb in its own process, alternating, and
# the medians across runs are compared. It fails only if a process cannot run.

require "fileutils"
require "json"
require "open3"
require "optparse"
require "rbconfig"
require "time"

ROOT = File.expand_path("..", __dir__)
METRICS = {
  "eval round trip p50 (us)" => [->(r) { r.dig("eval_rtt_ns", "p50")&./(1e3) }, :lower],
  "call round trip p50 (us)" => [->(r) { r.dig("call_rtt_ns", "p50")&./(1e3) }, :lower],
  "startup p50 (ms)" => [->(r) { r.dig("startup_ns", "p50")&./(1e6) }, :lower],
  "JS to Ruby (MB/s)" => [->(r) { r["js_to_ruby_mbps"].is_a?(Numeric) ? r["js_to_ruby_mbps"] : nil }, :higher],
  "Ruby to JS (MB/s)" => [->(r) { r["ruby_to_js_mbps"].is_a?(Numeric) ? r["ruby_to_js_mbps"] : nil }, :higher],
}.freeze
# Run-to-run spread above this marks a median as noisy.
NOISE = 0.10
ROLES = %w[reference candidate].freeze

options = { runs: 5, quick: false, cpus: "0-7" }
OptionParser.new do |opts|
  opts.on("--runs N", Integer) { |n| options[:runs] = n }
  opts.on("--quick", "One short run each, for a smoke test") { options.merge!(quick: true, runs: 1) }
  opts.on("--cpus LIST", "CPUs to pin with taskset when it exists") { |list| options[:cpus] = list }
end.parse!

def env(role)
  if role == "candidate"
    return { "BUNDLE_GEMFILE" => File.join(ROOT, "Gemfile"),
             "BUNDLE_WITHOUT" => ENV.fetch("BUNDLE_WITHOUT", nil), }
  end

  { "BUNDLE_GEMFILE" => File.join(ROOT, "tools/reference/Gemfile"),
    "BUNDLE_PATH" => File.join(ROOT, "tools/reference/vendor/bundle"), }
end

def measure(role, options)
  pin = system("command -v taskset >/dev/null 2>&1") ? ["taskset", "-c", options[:cpus]] : []
  args = [*pin, "bundle", "exec", "ruby", *(role == "candidate" ? ["-I#{File.join(ROOT, "lib")}"] : []),
          File.join(__dir__, "measure.rb"), role, *(options[:quick] ? ["--quick"] : []),]
  worker_env = env(role)
  run = -> { Open3.capture3(worker_env, *args, chdir: ROOT) }
  out, err, status = defined?(Bundler) ? Bundler.with_unbundled_env(&run) : run.call
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

governor = begin
  File.read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").strip
rescue Errno::ENOENT
  "unknown"
end
puts "#{raw["reference"].first["gem"]} against #{raw["candidate"].first["gem"]}, Ruby #{RUBY_VERSION}, " \
     "#{options[:runs]} run(s), medians, governor #{governor}"
puts
puts "| Metric | mini_racer | mini_racer_rs | Ratio | Note |"
puts "|---|---|---|---|---|"
summary = METRICS.to_h do |name, (read, direction)|
  values = ROLES.to_h { |role| [role, raw[role].map(&read)] }
  medians = values.transform_values { |list| list.compact.size == list.size ? median(list) : nil }
  reference, candidate = medians.values_at(*ROLES)
  ratio = (direction == :lower ? reference / candidate : candidate / reference) if reference && candidate
  noisy = values.values.any? { |list| list.compact.size > 1 && (list.compact.max / list.compact.min) - 1 > NOISE }
  note = if medians["candidate"]
           noisy ? "noisy" : ""
         else
           "unsupported"
         end
  cells = [medians["reference"], medians["candidate"]].map { |value| value ? format("%.2f", value) : "n/a" }
  puts "| #{name} | #{cells.join(" | ")} | #{ratio ? format("%.2fx", ratio) : "n/a"} | #{note} |"
  [name,
   { "reference" => medians["reference"], "candidate" => medians["candidate"], "ratio" => ratio, "noisy" => noisy },]
end

stamp = Time.now.utc.strftime("%Y%m%dT%H%M%SZ")
out = File.join(ROOT, "tmp", "bench", "#{stamp}.json")
FileUtils.mkdir_p(File.dirname(out))
File.write(out,
           JSON.pretty_generate({ "governor" => governor, "runs" => options[:runs], "summary" => summary,
                                  "raw" => raw, }),)
puts
puts "results: #{out.delete_prefix("#{ROOT}/")}"
