# frozen_string_literal: true

# One implementation's numbers, as JSON on stdout. bench/run.rb starts it once
# per implementation and run; a metric this gem cannot run yet is recorded as
# unsupported with the error instead of a number.

require "digest"
require "json"
require_relative "payload"

role = ARGV.fetch(0)
quick = ARGV.include?("--quick")
require "mini_racer"
loaded = defined?(MiniRacerRs) ? "candidate" : "reference"
abort "bench: started as #{role} but the #{loaded} gem loaded" unless role == loaded

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
def percentile(sorted, fraction) = sorted[((sorted.size - 1) * fraction).round]

def samples(count, warmup, &)
  warmup.times(&)
  Array.new(count) do
    started = now
    yield
    now - started
  end.sort
end

def metric
  yield
rescue StandardError, NotImplementedError => e
  { "unsupported" => "#{e.class}: #{e.message[0, 120]}" }
end

rounds = quick ? 2_000 : 100_000
result = {
  "gem" => defined?(MiniRacerRs) ? "mini_racer_rs #{MiniRacerRs::VERSION}" : "mini_racer #{MiniRacer::VERSION}",
}

result["eval_rtt_ns"] = metric do
  context = MiniRacer::Context.new
  times = samples(rounds, rounds / 10) { context.eval("1") }
  { "p50" => percentile(times, 0.5), "p99" => percentile(times, 0.99) }
end

result["call_rtt_ns"] = metric do
  context = MiniRacer::Context.new
  context.eval("function id(x) { return x }")
  times = samples(rounds, rounds / 10) { context.call("id", 1) }
  { "p50" => percentile(times, 0.5), "p99" => percentile(times, 0.99) }
end

# Neither side disposes inside the loop, since mini_racer_rs has no dispose yet,
# and GC is off while timing so no finalizer of an earlier context lands in a
# later sample. The count stays small because every context keeps its isolate.
result["startup_ns"] = metric do
  GC.start
  GC.disable
  5.times { MiniRacer::Context.new.eval("1+1") }
  times = samples(quick ? 10 : 50, 0) { MiniRacer::Context.new.eval("1+1") }
  { "p50" => percentile(times, 0.5), "p99" => percentile(times, 0.99) }
ensure
  GC.enable
  GC.start
end

props = BenchPayload.build
json = JSON.generate(props)
megabytes = json.bytesize / 1e6
result["payload_bytes"] = json.bytesize

result["js_to_ruby_mbps"] = metric do
  context = MiniRacer::Context.new
  context.eval("var props = #{json}")
  result["payload_digest"] = Digest::SHA256.hexdigest(Marshal.dump(context.eval("props")))
  megabytes / (percentile(samples(quick ? 5 : 30, 5) { context.eval("props") }, 0.5) / 1e9)
end

result["ruby_to_js_mbps"] = metric do
  context = MiniRacer::Context.new
  context.eval("function sink(x) { return 0 }")
  megabytes / (percentile(samples(quick ? 5 : 30, 5) { context.call("sink", props) }, 0.5) / 1e9)
end

puts JSON.generate(result)
