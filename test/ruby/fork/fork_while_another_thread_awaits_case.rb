# frozen_string_literal: true

# A runner awaiting a promise sleeps until a task is due, holding its isolate
# but running nothing in it, so a fork need not wait for it to finish: a
# context idle at the fork is taken over in the child, and the awaiting one
# stays the parent's.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

started_r, started_w = IO.pipe
awaiting = MiniRacer::Context.new
awaiting.attach("started", proc {
  started_w.write("x")
  started_w.flush
  nil
},)
idle = MiniRacer::Context.new
idle.eval("var answer = 42")
worker = Thread.new do
  raised(MiniRacer::ScriptTerminatedError) { awaiting.eval_await("started(); new Promise(() => {})") }
end
started_r.read(1)
child do
  raise "idle context" unless idle.eval("answer") == 42

  eval_error = raised(MiniRacer::RuntimeError) { awaiting.eval("1") }
  raise "awaiting context: #{eval_error.inspect}" unless eval_error&.start_with?("single-threaded runner:")
end
awaiting.stop
raise "await: #{worker.value.inspect}" unless worker.value == "terminated"
raise "parent idle context" unless idle.eval("answer") == 42
raise "parent awaiting context" unless awaiting.eval("1 + 1") == 2
