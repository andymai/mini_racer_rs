# frozen_string_literal: true

# The busy runner is interrupted in its loop and parks there for the fork;
# the child leaves that context and takes the idle one over.

require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

busy = MiniRacer::Context.new
idle = MiniRacer::Context.new
idle.eval("var answer = 42")
worker = Thread.new { busy.eval("const end = Date.now() + 1000; while (Date.now() < end) {}; 'done'") }
sleep 0.1
child do
  raise "idle" unless idle.eval("answer") == 42

  error = raised(MiniRacer::RuntimeError) { busy.eval("1") }
  raise "busy: #{error.inspect}" unless error&.start_with?("single-threaded runner:")
  raise "new context" unless MiniRacer::Context.new.eval("2") == 2
end
raise "worker" unless worker.value == "done"
