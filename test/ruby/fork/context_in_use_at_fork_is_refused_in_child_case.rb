# frozen_string_literal: true

require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

started_r, started_w = IO.pipe
release_r, release_w = IO.pipe
context = MiniRacer::Context.new
context.attach("block", proc {
  started_w.write("x")
  started_w.flush
  release_r.read(1)
  42
},)
worker = Thread.new { context.eval("block()") }
started_r.read(1)
child do
  eval_error = raised(MiniRacer::RuntimeError) { context.eval("1") }
  raise "eval: #{eval_error.inspect}" unless eval_error&.start_with?("single-threaded runner:")

  dispose_error = raised(MiniRacer::RuntimeError) { context.dispose }
  raise "dispose: #{dispose_error.inspect}" unless dispose_error&.start_with?("context dispose:")

  context.stop
  raise "new context" unless MiniRacer::Context.new.eval("1") == 1
end
release_w.write("x")
release_w.flush
raise "worker" unless worker.value == 42
