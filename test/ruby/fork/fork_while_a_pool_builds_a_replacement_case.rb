# frozen_string_literal: true

# A fork while the pool's own thread builds a replacement, its runner busy in
# JavaScript: the child, which has no such thread, builds its own, and the
# parent's build finishes.
require_relative "prelude"

built = []
pool = MiniRacerRs::Pool.new(size: 1, max_uses: 1) do |context|
  built << context
  context.eval("var answer = 42")
  context.eval("const end = Date.now() + 300; while (Date.now() < end) {}") if built.size == 2
end
pool.with { 1 }
sleep 0.01 until built.size == 2
sleep 0.05
child do
  raise "child answer" unless pool.with { _1.eval("answer") } == 42
end
raise "parent answer" unless pool.with { _1.eval("answer") } == 42
raise "the parent built #{built.size}" unless built.size >= 2
