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

  NIL_NODE = ["nil"].freeze
  REPEATS = %w[ref again].freeze
  LOST = [NIL_NODE, ["array", []]].freeze
  ERROR_FIELDS = %w[message stack cause].freeze
  SCALARS = %w[string int float bool].freeze

  def lost_reference?(expected, actual)
    LostReference.new(expected, actual).match?
  end

  # mini_racer numbers objects as V8 does not, so a back reference reaches the
  # wrong object in either direction (behavior:object-ids@ruby).
  #
  # Decoding, it resolves against a shorter list, in which each error also
  # holds the Hash its fields are collected in. Where the candidate returns an
  # object it already returned, the reference has nil, an object it already
  # returned, the empty Array it holds an error's place with, or that Hash. A
  # typed array, ArrayBuffer, RegExp or wrapper object reaches Ruby as a
  # String, number or boolean, which met again as a Hash key, or sent back by
  # a callback, is an equal value rather than the same object: there the
  # candidate has a value equal to an earlier one where the reference has nil
  # (for a String) or an object it already returned.
  #
  # Encoding, mini_racer numbers only Arrays and Hashes, so past a Time or a
  # Binary its back reference hands JavaScript another object of the
  # arguments, whose value moves: the reference has it where the candidate
  # returns an object again, and nil or an object it already returned where
  # the candidate has it.
  #
  # Every other part must be equal.
  class LostReference
    def initialize(expected, actual)
      @order = {}.compare_by_identity
      @scalars = []
      index(actual)
      @differences = differences(expected, actual, [])
      @moved_from = @differences.filter_map { |want, got| want if repeat?(got) && want != NIL_NODE && !repeat?(want) }
      @moved_to = @differences.filter_map { |want, got| got if misread?(want) && !repeat?(got) }
    end

    def match?
      @differences.all? { |want, got| explained?(want, got) }
    end

    private

    def explained?(want, got)
      if repeat?(got)
        LOST.include?(want) || repeat?(want) || error_fields?(want) || @moved_to.any? { alike?(want, _1) }
      elsif want == NIL_NODE
        earlier?(got, %w[string]) || moved?(got)
      elsif ref?(want)
        earlier?(got, SCALARS) || moved?(got)
      else
        false
      end
    end

    # The smallest parts where the two differ, with repeats compared whole.
    def differences(want, got, found)
      if want == got
        found
      elsif aligned?(want, got)
        want.zip(got) { |left, right| differences(left, right, found) }
        found
      else
        found << [want, got]
      end
    end

    def aligned?(want, got)
      want.is_a?(Array) && got.is_a?(Array) && want.size == got.size && !repeat?(want) && !repeat?(got)
    end

    def repeat?(node)
      node.is_a?(Array) && REPEATS.include?(node.first)
    end

    def ref?(node)
      node.is_a?(Array) && node.first == "ref"
    end

    def misread?(node)
      node == NIL_NODE || ref?(node)
    end

    def moved?(node)
      @moved_from.any? { alike?(_1, node) }
    end

    # Equal, except that any two repeats count as alike: a moved value's own
    # repeats are numbered in two different graphs.
    def alike?(left, right)
      return true if left == right || (repeat?(left) && repeat?(right))

      left.is_a?(Array) && right.is_a?(Array) && left.size == right.size &&
        left.zip(right).all? { |l, r| alike?(l, r) }
    end

    def error_fields?(node)
      return false unless node.is_a?(Array) && node.first == "hash" && !node[1].empty?

      keys = node[1].map { |key, _| key.first == "string" ? key.last : nil }
      keys == ERROR_FIELDS & keys
    end

    def earlier?(node, kinds)
      node.is_a?(Array) && kinds.include?(node.first) && @order.key?(node) &&
        @scalars.any? { |seen| @order[seen] < @order[node] && seen == node }
    end

    # Every node's place in DiffCanonical's walk, and the scalars met on it.
    def index(node)
      return unless node.is_a?(Array) && node.first.is_a?(String)

      @order[node] = @order.size
      case node.first
      when "array" then node[1].each { |child| index(child) }
      when "hash" then node[1].flatten(1).each { |child| index(child) }
      when "exception" then index(node[4])
      when "again" then index(node[1])
      when *SCALARS then @scalars << node
      end
    end
  end
end
