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
  LOST = [["nil"], ["array", []]].freeze

  def lost_reference?(expected, actual)
    LostReference.new(actual).match?(expected, actual)
  end

  # mini_racer resolves V8's back references against a shorter list, so where
  # the candidate returns an object it already returned, the reference has nil,
  # an object it already returned, the empty Array it holds an error's place
  # with, or a copy equal to the object referred to. A typed array, ArrayBuffer
  # or RegExp reaches Ruby as a String, which a callback sends back as a plain
  # value, so there the candidate has an equal String again where the reference
  # has nil. Every other part must be equal.
  class LostReference
    def initialize(actual)
      @containers = []
      @order = {}.compare_by_identity
      @strings = []
      index(actual)
    end

    def match?(expected, actual)
      return true if expected == actual
      return repeated?(expected, actual) if repeat?(actual)
      return true if expected == ["nil"] && earlier_string?(actual)
      return false unless expected.is_a?(Array) && actual.is_a?(Array) && expected.size == actual.size

      expected.zip(actual).all? { |want, got| match?(want, got) }
    end

    private

    def repeat?(node)
      node.is_a?(Array) && REPEATS.include?(node.first)
    end

    def repeated?(expected, actual)
      return true if LOST.include?(expected) || repeat?(expected)

      target = actual.first == "ref" && @containers[actual[1]]
      target ? match?(expected, target) : false
    end

    def earlier_string?(node)
      node.is_a?(Array) && node.first == "string" &&
        @strings.any? { |string| @order[string] < @order[node] && string == node }
    end

    # Containers numbered as DiffCanonical numbers them, and every node's place
    # in that walk.
    def index(node)
      return unless node.is_a?(Array) && node.first.is_a?(String)

      @order[node] = @order.size
      case node.first
      when "array" then number(node) { node[1].each { |child| index(child) } }
      when "hash" then number(node) { node[1].flatten(1).each { |child| index(child) } }
      when "exception" then number(node) { index(node[4]) }
      when "again" then index(node[1])
      when "string" then @strings << node
      end
    end

    def number(node)
      @containers << node
      yield
    end
  end
end
