# frozen_string_literal: true

# A fork waits out its quiesce bound behind a parse V8 does not interrupt,
# while another process sends SIGINT and a Ruby thread raises into the
# forking thread, both 300 ms in. fork() holds the GVL, so the trap and the
# exception land once it returns: then every context answers within a second,
# and after the parse a further fork's child takes the idle context over.
require_relative "prelude"

class Interrupted < StandardError
end

MiniRacer::Platform.set_flags!(:single_threaded)
idle = MiniRacer::Context.new
idle.eval("var answer = 42")
busy = MiniRacer::Context.new

# A parse that outlasts the 500 ms bound with room to spare.
pairs = 4_000_000
loop do
  busy.eval("globalThis.big = JSON.stringify(Array.from({length: #{pairs}}, (_, i) => [i, i]))")
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  busy.eval("JSON.parse(big).length")
  break if Process.clock_gettime(Process::CLOCK_MONOTONIC) - started > 1.0
  raise "no parse took a second" if pairs >= 64_000_000

  pairs *= 2
end

trapped = false
trap(:INT) { trapped = true }
parser = Thread.new { busy.eval("JSON.parse(big).length") }
sleep 0.05
main = Thread.current
raiser = Thread.new do
  sleep 0.3
  main.raise(Interrupted)
end
signaller = Process.spawn("sleep 0.3; kill -INT #{Process.pid}")

raised = false
held = nil
forked_at = Process.clock_gettime(Process::CLOCK_MONOTONIC)
begin
  fork { exit!(0) }
  held = Process.clock_gettime(Process::CLOCK_MONOTONIC) - forked_at
  sleep 1 # the exception may land only at the next interrupt check
rescue Interrupted
  raised = true
end
raiser.join
Process.wait(signaller)
Process.waitall
raise "the forking thread never got its exception" unless raised
raise "the fork returned after #{held} s, before its bound" if held && held < 0.45

started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
raise "idle" unless idle.eval("answer") == 42

waited = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
raise "the idle context answered after #{waited} s" if waited > 1
raise "parser" unless parser.value == pairs
raise "busy" unless busy.eval("1 + 1") == 2

sleep 0.1 until trapped || Process.clock_gettime(Process::CLOCK_MONOTONIC) - started > 2
raise "the INT trap never ran" unless trapped

child { raise "idle in child" unless idle.eval("answer") == 42 }
