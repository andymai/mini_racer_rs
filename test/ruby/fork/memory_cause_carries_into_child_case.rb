# frozen_string_literal: true

# A context idle at the fork keeps the memory cause a garbage collection left
# for its next script: the child's next eval runs out of memory, as the
# parent's does.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

context = MiniRacer::Context.new(max_memory: 100_000)
context.low_memory_notification
child do
  error = raised(MiniRacer::V8OutOfMemoryError) { context.eval("1") }
  raise "child: #{error.inspect}" unless error == "out of memory"
  raise "child: context unusable" unless context.eval("1 + 1") == 2
end
error = raised(MiniRacer::V8OutOfMemoryError) { context.eval("1") }
raise "parent: #{error.inspect}" unless error == "out of memory"
