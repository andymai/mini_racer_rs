# frozen_string_literal: true

# A forked child forgets the contexts its parent's pool built without
# disposing them, builds its own, and leaves the parent's pool as it was.
# Single-threaded, so the child can still take over an inherited context and
# show it was not disposed.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

built = []
pool = MiniRacerRs::Pool.new(size: 2) do |context|
  built << context
  context.eval("var answer = 42")
end
parents = built.dup
child do
  pair = pool.with { |first| Fiber.new { pool.with { |second| [first, second] } }.resume }
  raise "the child checked out its parent's contexts" if pair.any? { |context| parents.any? { _1.equal?(context) } }
  raise "the child built #{built.size - 2} contexts" unless built.size == 4
  raise "answer" unless pair.map { _1.eval("answer") } == [42, 42]
  raise "an inherited context was disposed" unless parents.map { _1.eval("answer") } == [42, 42]
end
held = pool.with { |first| Fiber.new { pool.with { |second| [first, second] } }.resume }
raise "the parent's pool changed" unless held.all? { |context| parents.any? { _1.equal?(context) } }
raise "the parent built more" unless built.size == 2
