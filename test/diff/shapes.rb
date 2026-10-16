# frozen_string_literal: true

# Accepted differences described by their shape rather than by regexes over
# one step's canonical outcomes.
module DiffShapes
  STRUCTURES = { "lost-reference" => :lost_reference? }.freeze

  module_function

  def match?(structure, expected, actual)
    method = STRUCTURES[structure]
    method ? send(method, expected, actual) : false
  end

  # mini_racer resolves V8's back references against a shorter list, so where
  # the candidate repeats a value it already returned, the reference has nil, a
  # value it already returned, or the empty Array it holds an error's place
  # with. Every other part must be equal.
  def lost_reference?(expected, actual)
    reference = []
    candidate = []
    record = lambda do |node, into|
      into << node
      node.each { |child| record.call(child, into) } if node.is_a?(Array)
    end
    walk = lambda do |want, got|
      repeat = got.is_a?(Array) && (got.first == "ref" || candidate.include?(got))
      lost = [["nil"], ["array", []]].include?(want) ||
             (want.is_a?(Array) && (want.first == "ref" || reference.include?(want)))
      if want == got || (repeat && lost)
        record.call(want, reference)
        record.call(got, candidate)
        next true
      end
      next false unless want.is_a?(Array) && got.is_a?(Array) && want.size == got.size

      reference << want
      candidate << got
      want.zip(got).all? { |pair| walk.call(*pair) }
    end
    walk.call(expected, actual)
  end
end
