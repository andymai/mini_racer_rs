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

def spread(times)
  { "p50" => percentile(times, 0.5), "p99" => percentile(times, 0.99), "p999" => percentile(times, 0.999) }
end

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
  spread(samples(rounds, rounds / 10) { context.eval("1") })
end

result["call_rtt_ns"] = metric do
  context = MiniRacer::Context.new
  context.eval("function id(x) { return x }")
  spread(samples(rounds, rounds / 10) { context.call("id", 1) })
end

# A timeout arms a watchdog around every call, which a plain call skips.
result["call_timeout_rtt_ns"] = metric do
  context = MiniRacer::Context.new(timeout: 60_000)
  context.eval("function id(x) { return x }")
  spread(samples(rounds, rounds / 10) { context.call("id", 1) })
end

# One sample is a JavaScript loop of callbacks, so the cost per callback is
# the round trip from V8 to Ruby and back, without a Ruby-side dispatch.
result["callback_rtt_ns"] = metric do
  context = MiniRacer::Context.new
  context.attach("echo", proc { |x| x })
  context.eval("globalThis.calls = (n) => { let s = 0; for (let i = 0; i < n; i++) s += echo(i); return s }")
  batch = quick ? 200 : 2_000
  times = samples(quick ? 5 : 50, 5) { context.call("calls", batch) }
  spread(times.map { |time| time / batch })
end

def startup(count, &)
  GC.start
  GC.disable
  times = samples(count, 3, &)
  spread(times)
ensure
  GC.enable
  GC.start
end

result["startup_ns"] = metric do
  startup(quick ? 10 : 50) do
    context = MiniRacer::Context.new
    context.eval("1+1")
    context.dispose
  end
end

babel = File.read(File.join(__dir__, "../vendor/mini_racer/benchmark/babel/helper_files/babel.js"))

result["babel_eval_startup_ns"] = metric do
  startup(quick ? 3 : 20) do
    context = MiniRacer::Context.new
    context.eval(babel)
    context.eval("typeof babel.transform")
    context.dispose
  end
end

result["babel_snapshot_startup_ns"] = metric do
  snapshot = MiniRacer::Snapshot.new(babel)
  result["babel_snapshot_bytes"] = snapshot.size
  startup(quick ? 3 : 20) do
    context = MiniRacer::Context.new(snapshot: snapshot)
    context.eval("typeof babel.transform")
    context.dispose
  end
end

# CPU time of the whole process, every runner thread included, while 50
# contexts that each ran one eval sit idle.
result["idle_cpu_percent"] = metric do
  contexts = Array.new(50) { MiniRacer::Context.new.tap { |context| context.eval("1") } }
  sleep 0.2
  window = quick ? 0.5 : 2.0
  started = Process.clock_gettime(Process::CLOCK_PROCESS_CPUTIME_ID)
  sleep window
  used = Process.clock_gettime(Process::CLOCK_PROCESS_CPUTIME_ID) - started
  contexts.each(&:dispose)
  100 * used / window
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
