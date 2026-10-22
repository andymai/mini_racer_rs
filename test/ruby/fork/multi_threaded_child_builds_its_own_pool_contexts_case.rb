# frozen_string_literal: true

# A multi-threaded child refuses every inherited context, so its pool builds
# its own on threads of its own, which a fork does not carry over, and renders
# from several threads on them.
require_relative "prelude"

built = []
pool = MiniRacerRs::Pool.new(size: 2) do |context|
  built << context
  context.eval("var answer = 42")
end
child do
  answers = Array.new(4) { Thread.new { pool.with { _1.eval("answer") } } }.map(&:value)
  raise "answers: #{answers.inspect}" unless answers == [42] * 4

  sleep 0.01 until built.size == 4
end
answers = Array.new(4) { Thread.new { pool.with { _1.eval("answer") } } }.map(&:value)
raise "parent answers: #{answers.inspect}" unless answers == [42] * 4
raise "the parent built more" unless built.size == 2
