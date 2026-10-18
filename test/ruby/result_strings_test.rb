# frozen_string_literal: true

require "test_helper"
require "mini_racer"

# Strings V8 sends as Latin-1 or UTF-16 come back as mini_racer returns them:
# UTF-8 Strings, unless they hold a lone surrogate, at every length around
# Ruby's embedded size and the decoder's ASCII blocks.
class ResultStringsTest < Minitest::Test
  LENGTHS = [0, 1, 22, 23, 24, 31, 32, 33, 63, 64, 65, 200, 5000].freeze

  def setup
    @context = MiniRacer::Context.new
  end

  def teardown
    @context.dispose
  end

  def strings(shapes)
    @context.eval("[#{LENGTHS}].flat(1).flatMap((n) => [#{shapes}])")
  end

  def assert_utf8(expected, actual)
    assert_equal expected, actual
    assert_equal Encoding::UTF_8, actual.encoding
    assert_predicate actual, :valid_encoding?
    assert_equal expected.ascii_only?, actual.ascii_only?
    refute_predicate actual, :frozen?
    assert_equal "#{expected}!", actual << "!"
  end

  def test_latin1_strings
    values = strings("'\\u00e9'.repeat(n), 'a'.repeat(n) + '\\u00e9', '\\u00e9' + 'b'.repeat(n)")
    expected = LENGTHS.flat_map { |n| ["é" * n, "#{"a" * n}é", "é#{"b" * n}"] }

    expected.zip(values) { |want, got| assert_utf8 want, got }
  end

  def test_two_byte_strings
    values = strings("'\\u0100'.repeat(n), '\\u{1F600}'.repeat(n), 'x'.repeat(n) + '\\u6771'")
    expected = LENGTHS.flat_map { |n| ["Ā" * n, "\u{1F600}" * n, "#{"x" * n}東"] }

    expected.zip(values) { |want, got| assert_utf8 want, got }
  end

  def test_strings_with_a_lone_surrogate_stay_utf16
    values = strings("'\\ud800' + 'y'.repeat(n), 'z'.repeat(n) + '\\udfff'")
    expected = LENGTHS.flat_map do |n|
      ["\x00\xD8#{"y\x00" * n}", "#{"z\x00" * n}\xFF\xDF"].map { |bytes| bytes.dup.force_encoding(Encoding::UTF_16LE) }
    end

    assert_equal expected, values
    assert(values.all? { |value| value.encoding == Encoding::UTF_16LE && !value.valid_encoding? })
  end

  # Text whose UTF-8 could pass 64 KiB is measured first and written into a
  # buffer of its own size: 32,768 Latin-1 bytes or 21,845 UTF-16 units could
  # just fit the decoder's own buffer, one more could not.
  def test_strings_measured_first
    values = @context.eval(<<~JS)
      [32768, 32769, 70000].flatMap((n) => ['\\u00e9'.repeat(n), 'a'.repeat(n) + '\\u00e9'])
        .concat([21845, 21846, 70000].flatMap((n) => ['\\u0100'.repeat(n), 'x'.repeat(n) + '\\u{1F600}']))
    JS
    expected = [32_768, 32_769, 70_000].flat_map { |n| ["é" * n, "#{"a" * n}é"] } +
               [21_845, 21_846, 70_000].flat_map { |n| ["Ā" * n, "#{"x" * n}\u{1F600}"] }

    expected.zip(values) { |want, got| assert_utf8 want, got }
    lone = @context.eval("'\\ud800' + 'y'.repeat(70000)")

    assert_equal "\x00\xD8#{"y\x00" * 70_000}".b, lone.b
    assert_equal Encoding::UTF_16LE, lone.encoding
  end

  def test_strings_inside_containers_and_patterns
    value = @context.eval(<<~JS)
      [{k: 'na\\u00efve \\u{1F600}'}, new String('\\u00e9t\\u00e9'), new RegExp('caf\\u00e9'), new Map([[1, '\\u6771']])]
    JS

    assert_utf8 "naïve \u{1F600}", value[0]["k"]
    assert_utf8 "été", value[1]
    assert_utf8 "café", value[2]
    assert_utf8 "東", value[3][1]
  end
end
