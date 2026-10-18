# frozen_string_literal: true

require "test_helper"
require "mini_racer"

# An object's keys and a map's string keys come back as mini_racer stores
# them: the interned String Hash#[]= keeps, in UTF-8 unless it holds a lone
# surrogate.
class ResultKeysTest < Minitest::Test
  def setup
    @context = MiniRacer::Context.new
  end

  def teardown
    @context.dispose
  end

  def interned?(key) = key.frozen? && key.equal?(-key)

  def test_keys_are_interned_and_values_are_not
    rows = @context.eval("[{id: 1, name: 'id'}, {id: 2, name: 'name'}]")

    assert_equal [{ "id" => 1, "name" => "id" }, { "id" => 2, "name" => "name" }], rows
    assert(rows.flat_map(&:keys).all? { |key| interned?(key) })
    assert_same rows[0].keys[0], rows[1].keys[0]
    refute_predicate rows[0]["name"], :frozen?
    refute_predicate rows[1]["name"], :frozen?
  end

  def test_keys_in_each_encoding
    value = @context.eval("({'caf\\u00e9': 1, '\\u6771': 2, '\\u{1F600}': 3, '\\ud800x': 4, 5: 6, 2.5: 7, a: 8})")

    lone = "\x00\xD8x\x00".dup.force_encoding(Encoding::UTF_16LE)

    assert_equal ["5", "café", "東", "\u{1F600}", lone, "2.5", "a"], value.keys
    assert_equal [Encoding::US_ASCII, *[Encoding::UTF_8] * 3, Encoding::UTF_16LE, *[Encoding::UTF_8] * 2],
                 value.keys.map(&:encoding)
    assert(value.keys.all? { |key| interned?(key) })
  end

  # Past the cache json's parser keeps for names in arrays: more names than
  # it holds, names too long for it, and names that do not start with a letter.
  def test_keys_past_the_cache_in_arrays
    rows = @context.eval(<<~JS)
      Array.from({length: 3}, (_, i) => Object.fromEntries([
        ...Array.from({length: 70}, (_, j) => ["key" + j, i]),
        ["x".repeat(55), i], ["y".repeat(56), i], ["_id", i], ["1a", i], ["", i], ["caf\\u00e9", i], ["\\u6771", i],
      ]))
    JS
    names = Array.new(70) { |j| "key#{j}" } + ["x" * 55, "y" * 56, "_id", "1a", "", "café", "東"]

    rows.each_with_index { |row, i| assert_equal names.to_h { |name| [name, i] }, row }
    assert(rows.flat_map(&:keys).all? { |key| interned?(key) })
    rows[0].keys.zip(rows[2].keys).each { |first, last| assert_same first, last }
  end

  def test_map_string_keys_are_interned_and_other_keys_kept
    map = @context.eval("new Map([['k', 1], [new String('s'), 2], [1, 3], ['\\udc00', 4], [null, 5]])")

    assert_equal({ "k" => 1, "s" => 2, 1 => 3, "\x00\xDC".dup.force_encoding(Encoding::UTF_16LE) => 4, nil => 5 }, map)
    assert(map.keys.grep(String).all? { |key| interned?(key) })
  end

  def test_a_key_that_reads_as_a_function_is_its_to_s
    key = @context.eval("({'\\u0BFFJavaScriptFunction': 1})").keys.first
    map_key = @context.eval("new Map([['\\u0BFFJavaScriptFunction', 1]])").keys.first

    assert_match(/\A#<MiniRacer::JavaScriptFunction:0x\h+>\z/, key)
    assert_instance_of MiniRacer::JavaScriptFunction, map_key
  end
end
