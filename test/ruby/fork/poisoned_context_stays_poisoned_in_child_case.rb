# frozen_string_literal: true

# A context whose heap stayed over its limit refuses JavaScript in a child
# that takes it over, as in its parent, until it is disposed.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
MiniRacerRs.heap_limit = 64 << 20
STILL_OVER = "heap still over its limit of #{64 << 20} bytes after a collection: " \
             "this context runs no more JavaScript".freeze

context = MiniRacer::Context.new
raised(MiniRacerRs::HeapLimitError) do
  context.eval("globalThis.keep = []; for (;;) keep.push({i: keep.length, s: 'k' + keep.length});")
end
# Each heap limit hit fills 64 MiB and collects in full, which on the slowest
# CI runners takes seconds.
child(within: 30) do
  error = raised(MiniRacerRs::HeapLimitError) { context.eval("1 + 1") }
  raise "child: #{error.inspect}" unless error == STILL_OVER

  context.dispose
  raise "child: new context unusable" unless MiniRacer::Context.new.eval("1 + 1") == 2
end
error = raised(MiniRacerRs::HeapLimitError) { context.eval("1 + 1") }
raise "parent: #{error.inspect}" unless error == STILL_OVER
