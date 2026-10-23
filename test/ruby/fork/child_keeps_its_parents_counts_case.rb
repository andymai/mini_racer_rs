# frozen_string_literal: true

# A child that takes an idle context over starts from the counts the context
# had at the fork, and from then on each process counts its own work.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

context = MiniRacer::Context.new
context.eval("var count = 1")
context.low_memory_notification
at_fork = MiniRacerRs.counters(context)
child do
  raise "child before: #{MiniRacerRs.counters(context)}" unless MiniRacerRs.counters(context) == at_fork
  raise "child eval" unless context.eval("++count") == 2

  context.low_memory_notification
  counts = MiniRacerRs.counters(context)
  raise "child dispatches: #{counts}" unless counts[:dispatches] == at_fork[:dispatches] + 2
  raise "child bytes: #{counts}" unless counts[:bytes_to_js] == at_fork[:bytes_to_js] + "++count".bytesize
  raise "child collections: #{counts}" unless counts[:gc_count] > at_fork[:gc_count]
end
raise "parent after the child: #{MiniRacerRs.counters(context)}" unless MiniRacerRs.counters(context) == at_fork
raise "parent eval" unless context.eval("++count") == 2
raise "parent dispatches" unless MiniRacerRs.counters(context)[:dispatches] == at_fork[:dispatches] + 1
