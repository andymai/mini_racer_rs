# frozen_string_literal: true

# A context idle at the fork comes to the child with the tasks V8 queued for
# it: a delayed task the parent's script left settles a promise in the child,
# as it does in the parent.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

context = MiniRacer::Context.new
context.eval(<<~JS)
  globalThis.done = false;
  Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 50).value.then(() => { done = true });
JS
child do
  sleep 0.1
  raise "child: no task ran" unless context.pump_message_loop
  raise "child: the promise did not settle" unless context.eval("done")
end
sleep 0.1
raise "parent: no task ran" unless context.pump_message_loop
raise "parent: the promise did not settle" unless context.eval("done")
