# frozen_string_literal: true

# A multi-threaded child refuses the contexts it inherited, and reads their
# counts as they were at the fork, from atomics alone.
require_relative "prelude"

context = MiniRacer::Context.new
context.eval("1")
at_fork = MiniRacerRs.counters(context)
child do
  error = raised(MiniRacer::RuntimeError) { context.eval("1") }
  raise "eval: #{error.inspect}" unless error&.start_with?("single-threaded runner:")
  raise "child: #{MiniRacerRs.counters(context)}" unless MiniRacerRs.counters(context) == at_fork
end
raise "parent eval" unless context.eval("2") == 2
raise "parent dispatches" unless MiniRacerRs.counters(context)[:dispatches] == at_fork[:dispatches] + 1
