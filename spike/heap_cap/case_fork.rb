# frozen_string_literal: true

# rubocop:disable Metrics/AbcSize, Metrics/MethodLength, Style/DocumentDynamicEvalDefinition

# A single-threaded context made in the parent, idle at a fork, taken over by
# the child, which hits the cap `repeat` times in it; then the parent hits it
# once in its own copy. Same output format as case.rb.
require "json"
require "mini_racer_rs"

workload = ARGV[0]
cap_mb = Integer(ARGV[1])
repeat = Integer(ARGV[2])
cap = cap_mb << 20
raise "objects only" unless workload == "objects"

JS = "(() => { const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]}); })()"

def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

def hit(context, index, cap)
  warn "HARNESS hit=#{index} begin"
  started = now
  outcome = begin
    context.eval(JS)
    { outcome: "completed" }
  rescue MiniRacer::V8OutOfMemoryError => e
    { outcome: "error", message: e.message }
  rescue StandardError => e
    { outcome: "other", error: e.class.to_s, message: e.message[0, 200] }
  end
  outcome[:seconds] = (now - started).round(4)
  outcome[:usable] = begin
    context.eval("1+1") == 2 ? "yes" : "wrong"
  rescue StandardError => e
    e.class.to_s
  end
  outcome[:probe] = begin
    context.eval("(() => { const a = []; for (let i = 0; i < #{cap / 4 / 64}; i++) a.push({i}); return a.length })()")
  rescue StandardError => e
    e.class.to_s
  end
  outcome[:pid] = Process.pid
  outcome[:hit] = index
  warn "HARNESS hit=#{index} end"
  puts JSON.generate(outcome)
end

$stdout.sync = true
MiniRacer::Platform.set_flags!(:single_threaded)
context = MiniRacer::Context.new
context.eval("1+1")
pid = fork do
  repeat.times { |index| hit(context, index, cap) }
  exit! 0
end
_, status = Process.wait2(pid)
puts JSON.generate(child_status: status.inspect)
hit(context, repeat, cap)
puts JSON.generate(later: [context.eval("1+1").to_s])
context.dispose
puts JSON.generate(done: true)
# rubocop:enable Metrics/AbcSize, Metrics/MethodLength, Style/DocumentDynamicEvalDefinition
