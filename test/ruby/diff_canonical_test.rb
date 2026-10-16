# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require_relative "../diff/canonical"
require_relative "../diff/generate"
require_relative "../diff/shrink"

class DiffCanonicalTest < Minitest::Test
  def test_every_generated_ruby_value_round_trips
    trees = DiffGenerate::RUBY_LEAVES + DiffGenerate.cases(seed: 7, javascript: 0, ruby: 50).flat_map { |kase| kase["args"] }

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

  def test_floats_and_strings_keep_what_a_comparison_needs
    refute_equal DiffCanonical.encode(0.0), DiffCanonical.encode(-0.0)
    assert_equal %w[float nan], DiffCanonical.encode(Float::NAN)
    assert_equal %w[string ASCII-8BIT hex ff], DiffCanonical.encode("\xFF".b)
    refute_equal DiffCanonical.encode("a"), DiffCanonical.encode("a".encode("UTF-16LE"))
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
    args = [%w[int 1], ["array", [%w[symbol s], ["hash", [[%w[int 7], %w[float nan]]]]]]]
    minimal = DiffShrink.minimize({ "kind" => "ruby", "args" => args }) do |variant|
      JSON.generate(variant["args"]).include?("nan")
    end

    assert_equal [%w[float nan]], minimal["args"]
  end
end
