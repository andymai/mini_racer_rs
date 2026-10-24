# frozen_string_literal: true

require "json"
require "test_helper"
require_relative "../diff/ledger"

module DiffShapesHelpers
  NOTHING = ["nil"].freeze
  ONE = ["array", [%w[int 1]]].freeze
  TWO = ["array", [%w[int 2]]].freeze
  TIME = ["time", "0", 0, 0, false].freeze
  BINARY = %w[binary 00].freeze
  FLOAT_ONE = %w[float 3ff0000000000000].freeze
  FLOAT_TWO = %w[float 4000000000000000].freeze
  SHAPE = { "id" => "lost", "compat" => "behavior:object-ids@ruby", "structure" => "lost-reference" }.freeze

  def text(value) = ["string", "UTF-8", "text", value]
  def int(value) = ["int", value.to_s]
  def list(*items) = ["array", items]
  def ref(number) = ["ref", number]
  def again(node) = ["again", node]
  def fields(*keys) = ["hash", keys.map { |key| [text(key), text("v")] }]

  # Outcomes come from JSON, so no two nodes are the same object.
  def reload(tree) = JSON.parse(JSON.generate(tree))

  def lost?(expected, actual)
    DiffShapes.match?("lost-reference", reload(expected), reload(actual))
  end

  def classify_steps(expected, actual)
    DiffLedger.classify(reload(expected), reload(actual), [SHAPE])
  end

  # By default the reference holds ONE in a first place and nil in a second; the candidate repeats an
  # object in the first and holds ONE in the second.
  def reference_move(value: ONE, second: NOTHING, before: []) = list(*before, list(TWO, value), second)
  def candidate_move(second, repeat: 3, before: []) = list(*before, list(TWO, ref(repeat)), second)
end

class DiffShapesErrorFieldsTest < Minitest::Test
  include DiffShapesHelpers

  ORDERED_FIELDS = [%w[message], %w[stack], %w[cause], %w[message stack], %w[message cause], %w[stack cause],
                    %w[message stack cause],].freeze

  def test_error_fields_are_accepted_at_a_repeat_in_every_ordered_subset
    ORDERED_FIELDS.each do |keys|
      assert lost?(list(ONE, fields(*keys)), list(ONE, ref(1))), "fields #{keys.join(", ")}"
    end
  end

  def test_a_hash_that_is_not_error_fields_is_not_accepted_at_a_repeat
    others = {
      "a plain hash" => ["hash", [[text("a"), int(1)]]],
      "an extra key after the fields" => fields("message", "stack", "extra"),
      "an extra key before the fields" => fields("extra", "message"),
      "fields out of order" => fields("stack", "message"),
      "a repeated field" => fields("message", "message"),
      "a key that is not a string" => ["hash", [[["symbol", text("message")], text("v")]]],
      "an empty hash" => ["hash", []],
      "an Array with items" => list(int(7)),
    }

    others.each { |label, node| refute lost?(list(ONE, node), list(ONE, ref(1))), label }
  end

  def test_error_fields_are_not_accepted_where_the_candidate_has_no_repeat
    refute lost?(list(ONE, fields("message")), list(ONE, NOTHING))
    refute lost?(list(ONE, fields("message")), list(ONE, int(5)))
  end
end

class DiffShapesEqualValueTest < Minitest::Test
  include DiffShapesHelpers

  def test_a_reference_is_accepted_against_a_value_equal_to_an_earlier_one
    values = { "string" => text("k"), "int" => int(7), "float" => FLOAT_ONE, "bool" => ["bool", true] }

    values.each { |kind, value| assert lost?(list(value, ref(0)), list(value, value)), "an earlier #{kind}" }
  end

  def test_a_repeated_hash_key_is_accepted_against_a_reference
    expected = ["hash", [[text("k"), ONE], [ref(0), TWO]]]

    assert lost?(expected, ["hash", [[text("k"), ONE], [text("k"), TWO]]])
    refute lost?(expected, ["hash", [[text("k"), ONE], [text("j"), TWO]]])
  end

  def test_a_reference_is_not_accepted_against_a_value_that_is_new
    pairs = {
      "another int" => [int(7), int(8)],
      "another string" => [text("k"), text("j")],
      "the same text in another encoding" => [text("k"), %w[string US-ASCII text k]],
      "another float" => [FLOAT_ONE, FLOAT_TWO],
      "another bool" => [["bool", true], ["bool", false]],
    }

    pairs.each { |label, (earlier, now)| refute lost?(list(earlier, ref(0)), list(earlier, now)), label }
  end

  def test_a_reference_is_not_accepted_against_an_equal_value_that_only_comes_later
    refute lost?(list(ref(0), int(7)), list(int(7), int(7)))
    refute lost?(list(ref(0), text("k")), list(text("k"), text("k")))
  end

  def test_a_reference_is_accepted_only_against_a_string_number_or_boolean
    symbol = ["symbol", text("a")]

    refute lost?(list(TIME, ref(0)), list(TIME, TIME))
    refute lost?(list(symbol, ref(0)), list(symbol, symbol))
    refute lost?(list(ONE, ref(1)), list(ONE, NOTHING))
  end

  def test_nil_is_accepted_against_an_earlier_string_but_not_an_earlier_number_or_boolean
    assert lost?(list(text("k"), NOTHING), list(text("k"), text("k")))
    [int(5), FLOAT_ONE, ["bool", false]].each do |value|
      refute lost?(list(value, NOTHING), list(value, value)), value.first
    end
  end

  def test_a_candidate_that_loses_an_identity_the_reference_keeps_is_not_accepted
    refute lost?(list(ONE, ref(1)), list(ONE, ONE)), "Array"
    refute lost?(list(text("x"), again(text("x"))), list(text("x"), text("x"))), "String"
    refute lost?(list(TIME, again(TIME)), list(TIME, TIME)), "Time"
    refute lost?(list(text("x"), again(text("x"))), list(text("x"), text("y"))), "another String"
  end
end

class DiffShapesMovedValueTest < Minitest::Test
  include DiffShapesHelpers

  def test_a_value_that_moves_is_accepted_when_one_place_gains_what_another_loses
    assert lost?(reference_move, candidate_move(ONE))
    assert lost?(reference_move(second: ref(0)), candidate_move(ONE)), "arriving where the reference has a reference"
    assert lost?(reference_move(before: [TIME, BINARY]), candidate_move(ONE, repeat: 4, before: [TIME, BINARY])),
           "after a Time and a Binary"
  end

  def test_a_value_that_moves_is_not_accepted_without_the_other_half
    refute lost?(reference_move(second: TWO), candidate_move(TWO)), "leaving without arriving"
    refute lost?(list(list(TWO), NOTHING), list(list(TWO), ONE)), "arriving without leaving"
    refute lost?(reference_move(second: text("x")), candidate_move(ONE)), "arriving where the reference has a String"
  end

  def test_a_move_pairs_within_one_step
    leaving = [reference_move(second: TWO), candidate_move(TWO)]
    arriving = [list(list(TWO), NOTHING), list(list(TWO), ONE)]

    assert_equal "accepted:lost", classify_steps([reference_move], [candidate_move(ONE)])
    assert_equal "diverged", classify_steps([leaving[0], arriving[0]], [leaving[1], arriving[1]])
  end

  def test_a_moved_value_must_arrive_unchanged
    refute lost?(reference_move, candidate_move(list(int(9)))), "another scalar"
    refute lost?(reference_move, candidate_move(list(int(1), int(1)))), "another size"
    refute lost?(reference_move, candidate_move(["hash", [[text("a"), int(1)]]])), "another kind"
  end

  def test_a_moved_values_own_repeats_may_be_numbered_differently
    moving = reference_move(value: list(ref(0)))

    assert lost?(moving, candidate_move(list(ref(5)))), "another number"
    assert lost?(moving, candidate_move(list(again(text("s"))))), "again for a reference"
    refute lost?(moving, candidate_move(list(int(1)))), "a value where it has a repeat"
  end

  def test_nil_and_a_reference_cannot_trade_places
    refute lost?(list(ONE, NOTHING, ref(0)), list(ONE, ref(0), NOTHING))
  end
end

class DiffShapesDivergenceTest < Minitest::Test
  include DiffShapesHelpers

  def test_a_difference_elsewhere_in_the_step_is_not_excused
    assert lost?(list(ONE, NOTHING, int(3)), list(ONE, ref(1), int(3)))
    assert lost?(list(reference_move, int(3)), list(candidate_move(ONE), int(3)))
    refute lost?(list(ONE, NOTHING, int(3)), list(ONE, ref(1), int(4))), "a lost reference"
    refute lost?(list(ONE, fields("message"), int(3)), list(ONE, ref(1))), "error fields"
    refute lost?(list(reference_move, int(3)), list(candidate_move(ONE), int(4))), "a move"
    refute lost?(list(ONE, NOTHING), list(ONE)), "a missing item"
  end

  def test_the_recorded_divergence_accepts_the_extended_mechanisms
    divergences = DiffLedger.load_divergences(File.expand_path("../diff/divergences.toml", __dir__))
    classify = ->(expected, actual) { DiffLedger.classify([reload(expected)], [reload(actual)], divergences) }
    plain = ["hash", [[text("a"), int(1)]]]

    assert_equal "accepted:reference-to-a-numbered-object",
                 classify.call(list(ONE, fields("message", "stack")), list(ONE, ref(1)))
    assert_equal "accepted:reference-to-a-numbered-object",
                 classify.call(list(text("k"), ref(0)), list(text("k"), text("k")))
    assert_equal "diverged", classify.call(list(ONE, plain), list(ONE, ref(1)))
  end
end
