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
replacement = built[1]
child do
  context, answer = pool.with { [_1, _1.eval("answer")] }
  raise "the child took the replacement its parent was building" if context.equal?(replacement)
  raise "child answer #{answer.inspect}" unless answer == 42
end
context, answer = pool.with { [_1, _1.eval("answer")] }
raise "the parent did not get the replacement it was building" unless context.equal?(replacement)
raise "parent answer #{answer.inspect}" unless answer == 42
