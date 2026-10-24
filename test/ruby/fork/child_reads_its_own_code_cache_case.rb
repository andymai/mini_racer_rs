# frozen_string_literal: true

# A forked child reads its own process's code cache: the entries and shared
# sources it inherited, and counts of its own work alone, while its parent's
# stay as they were. mini_racer 0.22.2 has no counterpart.
require_relative "prelude"

source = "/*#{"x" * (1 << 20)}*/ 41 + 1"
3.times { MiniRacer::Context.new.eval(source) }
at_fork = MiniRacerRs.code_cache
unless at_fork.values_at(:entries, :builds, :hits, :shared_sources) == [1, 1, 1, 1]
  raise "parent before the fork: #{at_fork}"
end

child do
  inherited = at_fork.merge(hits: 0, builds: 0, rebuilds: 0, refused: 0)
  raise "child before: #{MiniRacerRs.code_cache}" unless MiniRacerRs.code_cache == inherited
  raise "child eval" unless MiniRacer::Context.new.eval(source) == 42
  raise "child after: #{MiniRacerRs.code_cache}" unless MiniRacerRs.code_cache == inherited.merge(hits: 1)
end
raise "parent after the child: #{MiniRacerRs.code_cache}" unless MiniRacerRs.code_cache == at_fork
