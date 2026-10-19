# frozen_string_literal: true

require "objspace"
require "test_helper"
require "mini_racer"

module CallTestNamespaceLongEnoughToCut
  module SecondLevel
    Unsendable = Class.new
  end
end

class CallTest < Minitest::Test
  include RubyProcess

  def setup
    @context = MiniRacer::Context.new
    @context.eval(<<~JS)
      globalThis.id = (...a) => a
      globalThis.one = () => 1
      globalThis.a = {b: {k: 2, c(x) { return x * this.k }}}
    JS
  end

  def teardown
    @context&.dispose
  end

  def test_calls_through_a_dotted_path_on_its_parent
    assert_equal 42, @context.call("a.b.c", 21)
    error = assert_raises(MiniRacer::RuntimeError) { @context.call("a.b.missing") }
    assert_equal "TypeError: not a function", error.message
    assert_raises(ArgumentError) { @context.call }
  end

  # mini_racer reads the name as UTF-8, so a lone surrogate becomes U+FFFD.
  def test_a_name_with_a_lone_surrogate_finds_the_replacement_character
    @context.eval("globalThis['\\ud800'] = () => 'surrogate'; globalThis['\\ufffd'] = () => 'replacement'")

    assert_equal "replacement", @context.call([0xD800].pack("v").force_encoding("UTF-16LE"))
  end

  # mini_racer sends these encodings' bytes as UTF-8, whatever Ruby knows of
  # them. A String Ruby has scanned goes by the coderange it keeps, and one it
  # has not is read; both arrive as the characters V8's UTF-8 decoding makes,
  # alone and in a hash V8 reads.
  def test_strings_sent_as_utf8_arrive_alike_whether_or_not_ruby_has_scanned_them
    @context.eval(<<~JS)
      globalThis.points = (...a) => a.map((s) => Array.from(s, (c) => c.codePointAt(0)))
      globalThis.valuePoints = (h) => points(...Object.values(h))
    JS
    texts = ["", "hello", "a\0b", "café", "東京", "\u{1F600}", "a\xFFb", "the quick brown fox é",
             "the quick brown fox 東京", "the quick brown fox \u{1F600}", "the quick brown fox \xE6\x9D",]
    expected = texts.map { |text| text.scrub("�").codepoints }
    states = %i[scanned unscanned]
    %w[UTF-8 US-ASCII ASCII-8BIT Windows-1252].each do |encoding|
      states.each do |state|
        strings = texts.map { |text| send(state, text.b.force_encoding(encoding)) }
        keyed = strings.each_with_index.to_h { |text, i| ["k#{i}", text] }
        if state == :unscanned
          assert(strings.drop(1).all? { |text| ObjectSpace.dump(text).include?('"coderange":"unknown"') })
        end

        assert_equal expected, @context.call("points", *strings), "#{encoding} #{state}"
        assert_equal expected, @context.call("valuePoints", keyed), "#{encoding} #{state} in a hash"
      end
    end
  end

  def test_unsupported_values_raise_mini_racers_cut_message
    error = assert_raises(MiniRacer::RuntimeError) do
      @context.call("id", CallTestNamespaceLongEnoughToCut::SecondLevel::Unsendable.new)
    end

    assert_equal "Context.call: unsupported type CallTestNamespaceLongEnoughToCut::SecondLevel:", error.message
    assert_equal Encoding::BINARY, error.message.encoding
  end

  def test_array_elements_are_read_as_the_encoder_reaches_them
    array = nil
    stamp = Object.new
    stamp.define_singleton_method(:to_time) do
      array[1] = "changed"
      array.pop
      Time.at(0)
    end
    array = [stamp, "orig", "third"]

    assert_equal [[Time.at(0), "changed", nil]], @context.call("id", array)
  end

  def test_an_argument_nested_past_v8s_limit_raises_an_internal_error
    deep = []
    20_000.times { deep = [deep] }
    error = assert_raises(MiniRacer::InternalError) { @context.call("one", deep) }

    assert_match(/Maximum call stack size exceeded/, error.message)
  end

  # mini_racer 0.22.1 reads a call's arguments once, so with a max_memory: they
  # fit into once, each of these calls returns 0. Values the runner built and
  # then left to V8 would be held while V8 read them all again.
  def test_a_call_left_to_v8_holds_its_arguments_once_under_max_memory
    strings = Array.new(1_000_000) { |i| format("s%06d", i) }
    cycle = []
    cycle << cycle
    [{ (+"\xE9").force_encoding("ISO-8859-1") => 1 }, cycle].each do |trigger|
      context = MiniRacer::Context.new(max_memory: 48_000_000)
      context.eval("globalThis.sink = () => 0")

      assert_equal 0, context.call("sink", strings, trigger)
    ensure
      context&.dispose
    end
  end

  def test_symbols_are_sent_by_name_and_datetime_need_not_be_a_class
    out, err, status = run_ruby(<<~RUBY)
      module DateTime; end
      require "mini_racer"
      class Symbol; def to_s = "patched"; end
      context = MiniRacer::Context.new
      context.eval("globalThis.id = (...a) => a")
      p context.call("id", :hello)
    RUBY

    assert_predicate status, :success?, err
    assert_equal "[\"hello\"]\n", out
  end

  private

  def scanned(text) = text.tap(&:valid_encoding?)

  # Setting a byte past ASCII and back leaves Ruby's coderange unknown.
  def unscanned(text)
    byte = text.getbyte(0) or return text
    text.setbyte(0, byte ^ 0x80)
    text.setbyte(0, byte)
    text
  end
end
