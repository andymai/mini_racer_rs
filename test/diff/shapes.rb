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

  REPEATS = %w[ref again].freeze

  # mini_racer resolves V8's back references against a shorter list, so where
  # the candidate returns an object it already returned, the reference has nil,
  # an object it already returned, or the empty Array it holds an error's place
  # with. Every other part must be equal.
  def lost_reference?(expected, actual)
    return true if expected == actual
    return lost?(expected) if repeat?(actual)
    return false unless expected.is_a?(Array) && actual.is_a?(Array) && expected.size == actual.size

    expected.zip(actual).all? { |want, got| lost_reference?(want, got) }
  end

  def repeat?(node)
    node.is_a?(Array) && REPEATS.include?(node.first)
  end

  def lost?(node)
    [["nil"], ["array", []]].include?(node) || repeat?(node)
  end
end
