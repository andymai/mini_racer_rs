# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require_relative "../diff/canonical"
require_relative "../diff/generate"
require_relative "../diff/shrink"

class DiffCanonicalTest < Minitest::Test
  def test_every_generated_ruby_value_round_trips
    trees = DiffGenerate::RUBY_LEAVES + DiffGenerate.cases(seed: 7, javascript: 0, ruby: 50).flat_map { |kase| kase["args"].map { |arg| DiffGenerate.canonical(arg) } }

    trees.each { |tree| assert_equal tree, DiffCanonical.encode(DiffCanonical.decode(tree)) }
  end

  def test_shared_containers_keep_their_identity
    shared = [1]
    encoded = DiffCanonical.encode([shared, { "a" => shared }])

    assert_equal ["array", [["array", [%w[int 1]]], ["hash", [[%w[string UTF-8 text a], ["ref", 1]]]]]],
                 encoded
    decoded = DiffCanonical.decode(encoded)

    assert_same decoded[0], decoded[1]["a"]
  end

  def test_the_same_string_or_time_returned_twice_is_marked_without_shifting_refs
    text = +"a"
    time = Time.at(0).utc
    shared = [1]
    encoded = DiffCanonical.encode([text, text, time, time, "k", "k", shared, shared])

    assert_equal %w[string UTF-8 text a], encoded[1][0]
    assert_equal ["again", %w[string UTF-8 text a]], encoded[1][1]
    assert_equal ["again", ["time", "0", 0, 0, true]], encoded[1][3]
    assert_equal %w[string UTF-8 text k], encoded[1][5]
    assert_equal ["ref", 1], encoded[1][7]
  end

  def test_floats_strings_symbols_and_times_keep_what_a_comparison_needs
    refute_equal DiffCanonical.encode(0.0), DiffCanonical.encode(-0.0)
    assert_equal %w[float nan], DiffCanonical.encode(Float::NAN)
    assert_equal %w[string ASCII-8BIT hex ff], DiffCanonical.encode("\xFF".b)
    refute_equal DiffCanonical.encode("a"), DiffCanonical.encode("a".encode("UTF-16LE"))
    refute_equal DiffCanonical.encode(:a), DiffCanonical.encode("a".encode("UTF-16LE").to_sym)
    refute_equal DiffCanonical.encode(Time.at(0).utc), DiffCanonical.encode(Time.at(0).localtime(0))
  end

  def test_an_error_message_keeps_its_encoding_and_stays_json_safe
    failure = DiffCanonical.failure(RuntimeError.new("\xFF".b))

    assert_equal ["raise", "RuntimeError", %w[string ASCII-8BIT hex ff]], failure
    assert JSON.generate(failure)
  end

  def test_generation_is_deterministic_per_seed
    assert_equal DiffGenerate.cases(seed: 3, javascript: 20, ruby: 20),
                 DiffGenerate.cases(seed: 3, javascript: 20, ruby: 20)
    refute_equal DiffGenerate.cases(seed: 3, javascript: 20, ruby: 0),
                 DiffGenerate.cases(seed: 4, javascript: 20, ruby: 0)
  end

  def test_shared_values_render_as_one_object_twice
    assert_equal "(() => { const s = ({\"a\": 1}); return [s, s]; })()",
                 DiffGenerate.render(["shared", ["object", [["a", %w[lit 1]]]]])
  end

  def test_shrinking_keeps_only_what_still_diverges
    tree = ["array", [%w[lit 1], ["object", [["a", ["set", [%w[lit NaN], %w[lit 2]]]]]], %w[lit 3]]]
    minimal = DiffShrink.minimize({ "kind" => "js", "tree" => tree }) do |variant|
      DiffGenerate.render(variant["tree"]).include?("NaN")
    end

    assert_equal %w[lit NaN], minimal["tree"]
  end

  def test_shrinking_ruby_arguments_keeps_one_argument
    args = [%w[int 1],
            ["array", [["symbol", %w[string US-ASCII text s]], ["hash", [[%w[int 7], %w[float nan]]]]]],]
    minimal = DiffShrink.minimize({ "kind" => "ruby", "args" => args }) do |variant|
      JSON.generate(variant["args"]).include?("nan")
    end

    assert_equal [%w[float nan]], minimal["args"]
  end

  def test_a_shared_value_shrinks_and_stays_shared
    shared = ["shared", ["array", [%w[int 1], ["array", [%w[int 2]]]]]]
    minimal = DiffShrink.minimize({ "kind" => "ruby", "args" => [shared] }) do |variant|
      variant["args"].first.first == "shared"
    end

    assert_equal [["shared", ["array", []]]], minimal["args"]
    assert_equal ["array", [["array", []], ["ref", 1]]], DiffGenerate.canonical(minimal["args"].first)
  end
end
