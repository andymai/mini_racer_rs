# frozen_string_literal: true

# A capped context idle at the fork is taken over with its limit: V8's
# callback names the child's own control block, so each hit there raises and
# the context carries on, in the child and in the parent. Contexts the child
# makes take the limit it inherited.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
MiniRacerRs.heap_limit = 64 << 20
OBJECTS = "(() => { const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]}); })()"
REACHED = "heap limit of #{64 << 20} bytes reached".freeze

context = MiniRacer::Context.new
context.eval("1 + 1")
child do
  3.times do
    error = raised(MiniRacerRs::HeapLimitError) { context.eval(OBJECTS) }
    raise "child: #{error.inspect}" unless error == REACHED
  end
  raise "child: context unusable" unless context.eval("1 + 1") == 2
  raise "child: limit not inherited" unless MiniRacerRs.heap_limit == 64 << 20

  fresh = MiniRacer::Context.new
  raise "child: new context uncapped" unless fresh.heap_stats[:heap_size_limit] == 64 << 20

  error = raised(MiniRacerRs::HeapLimitError) { fresh.eval(OBJECTS) }
  raise "child: new context #{error.inspect}" unless error == REACHED
end
error = raised(MiniRacerRs::HeapLimitError) { context.eval(OBJECTS) }
raise "parent: #{error.inspect}" unless error == REACHED
raise "parent: context unusable" unless context.eval("1 + 1") == 2
