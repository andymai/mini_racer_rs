# frozen_string_literal: true

# rubocop:disable Metrics/AbcSize, Metrics/CyclomaticComplexity, Metrics/MethodLength, Metrics/BlockLength, Layout/LineLength

# One child process: a context under the heap cap set by the environment, a
# workload run `repeat` times, each followed by checks. One JSON line per hit
# on stdout; markers on stderr so the parent can pair them with HEAPCAP lines.
require "json"
require "mini_racer_rs"

workload = ARGV[0]
cap_mb = Integer(ARGV[1])
repeat = Integer(ARGV[2])
cap = cap_mb << 20
opts = {}
opts[:max_memory] = Integer(ENV["CASE_MAX_MEMORY"]) if ENV["CASE_MAX_MEMORY"]
opts[:timeout] = Integer(ENV["CASE_TIMEOUT_MS"]) if ENV["CASE_TIMEOUT_MS"]

def js_for(workload, cap)
  case workload
  when "objects"
    "(() => { const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]}); })()"
  when "microtask_after"
    loop_js = "const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]});"
    "Promise.resolve().then(() => { #{loop_js} }); (() => { #{loop_js} })()"
  when "finally_alloc"
    loop_js = "const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]});"
    "(() => { try { #{loop_js} } finally { #{loop_js} } })()"
  when "catch_retry"
    loop_js = "const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]});"
    "(() => { for (;;) { try { (() => { #{loop_js} })() } catch (e) {} } })()"
  when "objects_global"
    "globalThis.keep = []; for (;;) keep.push({i: keep.length, s: 'k' + keep.length, n: [keep.length, 1]});"
  when "concat"
    "(() => { let s = 'abcdefghijklmno\\u0100'; for (;;) { s = s + s; s.charCodeAt(s.length - 1); } })()"
  when "json", "json_mild"
    unit = '{"a":12345,"b":"xy","c":[1,2]}'
    n = (cap * (workload == "json" ? 0.4 : 0.25) / (unit.bytesize + 1)).to_i
    "(() => { const unit = '#{unit}'; const s = '[' + (unit + ',').repeat(#{n}) + unit + ']'; return JSON.parse(s).length; })()"
  when "literal"
    items = (0...10_000).map { |i| "{a:#{i}}" }.join(",")
    "(() => { const keep = []; const f = () => [#{items}]; for (;;) keep.push(f()); })()"
  when "literal_single"
    n = (cap * 1.2 / 80).to_i
    "[#{"{}," * n}{}].length"
  when "map"
    "(() => { const m = new Map(); for (let i = 0; ; i++) m.set(i, {i}); })()"
  when "recursion"
    k = [cap / (8 * 2000), 16].max
    "(() => { function f(d) { const a = new Array(#{k}).fill(d); return [a, f(d + 1)]; } return f(0); })()"
  when "arraybuffer"
    t = 2 * cap
    "(() => { const keep = []; let total = 0; while (total < #{t}) { keep.push(new Float64Array(2 << 20)); keep.push(new ArrayBuffer(8 << 20)); total += 24 << 20; } return total; })()"
  when "big_source"
    # A comment-heavy source past 1 MiB, which shares as an external string.
    "function f() { return 1 }\n// #{"x" * (cap + (cap / 4))}\nf()"
  else
    abort "unknown workload #{workload}"
  end
end

def probe_js(cap)
  n = cap / 4 / 64
  "(() => { const a = []; for (let i = 0; i < #{n}; i++) a.push({i}); return a.length })()"
end

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

js = js_for(workload, cap)
MiniRacer::Platform.set_flags!(:single_threaded) if ENV["CASE_SINGLE_THREADED"] == "1"
MiniRacer::Platform.set_flags!(max_old_space_size: Integer(ENV["CASE_MAX_OLD_SPACE"])) if ENV["CASE_MAX_OLD_SPACE"]
context = MiniRacer::Context.new(**opts)
$stdout.sync = true
repeat.times do |hit|
  warn "HARNESS hit=#{hit} begin"
  started = now
  outcome = begin
    value = context.eval(js)
    { outcome: "completed", value: value.is_a?(Numeric) ? value : value.class.to_s }
  rescue MiniRacer::V8OutOfMemoryError => e
    { outcome: "error", message: e.message }
  rescue StandardError => e
    { outcome: "other", error: e.class.to_s, message: e.message[0, 200] }
  end
  outcome[:seconds] = (now - started).round(4)
  stats = context.heap_stats
  outcome[:used_after] = stats[:used_heap_size]
  outcome[:limit_after] = stats[:heap_size_limit]
  outcome[:external_after] = stats[:external_memory]
  outcome[:usable] = begin
    context.eval("1+1") == 2 ? "yes" : "wrong"
  rescue StandardError => e
    e.class.to_s
  end
  outcome[:probe] = begin
    context.eval(probe_js(cap))
  rescue StandardError => e
    e.class.to_s
  end
  outcome[:hit] = hit
  outcome[:rss_kb] = File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
  warn "HARNESS hit=#{hit} end"
  puts JSON.generate(outcome)
  break if outcome[:usable] != "yes"
end
later = Array.new(5) do
  context.eval("1+1").to_s
rescue StandardError => e
  e.class.to_s
end
puts JSON.generate(later: later)
context.dispose
puts JSON.generate(done: true)
# rubocop:enable Metrics/AbcSize, Metrics/CyclomaticComplexity, Metrics/MethodLength, Metrics/BlockLength, Layout/LineLength
