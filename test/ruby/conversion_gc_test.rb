# frozen_string_literal: true

require_relative "test_helper"
require "mini_racer"

# Converts values full of back references while every allocation starts a
# compacting collection, so a value the decoder or encoder keeps where Ruby's
# GC cannot see it is freed or moved before it is used.
module ConversionGc
  # Moves every movable object while the encoder is part way through the
  # arguments.
  class Stamp
    def to_time
      GC.verify_compaction_references(expand_heap: true, toward: :empty)
      Time.at(0)
    end
  end

  def setup
    skip "needs CRuby's GC.stress and compaction" unless RUBY_ENGINE == "ruby"
    @context = MiniRacer::Context.new
  end

  def teardown
    @context&.dispose
  end

  private

  def stressed
    compacting = GC.auto_compact
    GC.auto_compact = true
    GC.stress = true
    yield
  ensure
    GC.stress = false
    GC.auto_compact = compacting
    GC.verify_compaction_references(expand_heap: true, toward: :empty)
  end
end

class ConversionGcTest < Minitest::Test
  include ConversionGc

  SOURCE = <<~JS
    (() => {
      const shared = {name: "shared", list: [1, "two", 3.5]}
      const cyclic = {label: "cyclic"}
      cyclic.self = cyclic
      const error = new Error("boom")
      error.cause = error
      const date = new Date(0)
      return {
        a: shared, b: shared, cyclic, date, again: date, error,
        map: new Map([[shared, "key"]]), set: new Set([shared]), text: "caf\\u00e9 \\u6771",
      }
    })()
  JS

  def test_back_references_survive_collections_and_moves
    value = stressed { @context.eval(SOURCE) }

    assert_equal({ "name" => "shared", "list" => [1, "two", 3.5] }, value["a"])
    assert_same value["a"], value["b"]
    assert_same value["a"], value["map"].keys.first
    assert_same value["a"], value["set"].first
    assert_same value["cyclic"], value["cyclic"]["self"]
    assert_same value["date"], value["again"]
    assert_same value["error"], value["error"].cause
    assert_equal "café 東", value["text"]
  end

  # Arrays and objects much longer than the decoder's batch, of values made
  # fresh while decoding, so every batch waits on the native stack through
  # collections and moves.
  def test_long_arrays_and_objects_survive_collections_and_moves
    value = stressed do
      @context.eval(<<~JS)
        (() => {
          const strings = Array.from({length: 200}, (_, i) => "item " + i)
          const object = Object.fromEntries(strings.map((s, i) => [s, [i, s]]))
          const texts = Object.fromEntries(strings.map((s) => [s, s + "!"]))
          const holey = new Array(200)
          for (let i = 0; i < 200; i++) holey[i] = "hole " + i
          const numbers = Array.from({length: 200}, (_, i) => 2 ** 70 + i * 2 ** 20)
          const tiny = Array.from({length: 200}, (_, i) => (i + 1) * 1e-300)
          const holeyNumbers = new Array(200)
          for (let i = 0; i < 200; i++) holeyNumbers[i] = i % 3 ? numbers[i] : tiny[i]
          return {strings, object, texts, holey, numbers, tiny, holeyNumbers}
        })()
      JS
    end
    numbers = Array.new(200) { |i| (2**70) + (i * (2**20)) }
    tiny = Array.new(200) { |i| (i + 1) * 1e-300 }

    assert_equal Array.new(200) { |i| "item #{i}" }, value["strings"]
    assert_equal(Array.new(200) { |i| ["item #{i}", [i, "item #{i}"]] }.to_h, value["object"])
    assert_equal(Array.new(200) { |i| ["item #{i}", "item #{i}!"] }.to_h, value["texts"])
    assert_equal Array.new(200) { |i| "hole #{i}" }, value["holey"]
    assert_equal numbers, value["numbers"]
    # Too small to be flonums, so each is allocated while its batch waits.
    assert_equal tiny, value["tiny"]
    assert_equal Array.new(200) { |i| (i % 3).zero? ? tiny[i] : numbers[i] }, value["holeyNumbers"]
    assert value["object"].keys.all?(&:frozen?)
    assert value["texts"].keys.all?(&:frozen?)
  end

  # Catches an unpinned value only in an optimized build, since a debug build's
  # stale stack copies pin values too; CI builds release. Skipped on macOS,
  # where Ruby 3.4's verification raises a malloc overflow after a few heap
  # doublings in one process.
  def test_arguments_survive_collections_and_moves_while_encoding
    if RUBY_PLATFORM.include?("darwin")
      skip "GC.verify_compaction_references overflows on macOS after repeated heap doublings"
    end

    @context.eval(<<~JS)
      globalThis.shape = (...a) => [
        a.length, a[1] === a[2], a[1].list[1], a[1].more[1].deep, a[3].other[1], a[4],
        a[0] instanceof Date, a[1].list[0] instanceof Date,
      ]
    JS
    result = stressed { @context.call("shape", *arguments) }

    assert_equal [5, true, [1, "two"], ["x"], [3, "four"], "caf\u00e9", true, true], result
  end

  # Only the context holds the callable, and its arguments and result exist
  # only while the callback runs.
  def test_callbacks_survive_collections_and_moves
    @context.attach("pair", proc { |a, b| [{ "a" => a, "b" => b, "text" => +"caf\u00e9" }, a] })
    GC.verify_compaction_references(expand_heap: true, toward: :empty)
    result = stressed do
      @context.eval("(() => { const s = {k: [1, 'two']}; const r = pair(s, s); return [r, r[0].a === r[1]] })()")
    end

    shared = { "k" => [1, "two"] }

    assert_equal [[{ "a" => shared, "b" => shared, "text" => "caf\u00e9" }, shared], true], result
  end

  private

  # Built here so the nested values are reachable only through their parents,
  # which leaves them free to move; each Stamp moves them before its siblings
  # are encoded.
  def arguments
    shared = { "list" => [Stamp.new, [1, +"two"]], "more" => [Stamp.new, { "deep" => [+"x"] }] }
    [Stamp.new, shared, shared, { "other" => [Stamp.new, [3, +"four"]] }, +"caf\u00e9"]
  end
end

# Immediates are written before anything is pinned, and the leading ones of an
# array or hash as it is reached; the Stamps then move everything that waits
# for its turn.
class ConversionGcImmediatesTest < Minitest::Test
  include ConversionGc

  def test_immediates_written_before_pinned_values_survive_collections_and_moves
    if RUBY_PLATFORM.include?("darwin")
      skip "GC.verify_compaction_references overflows on macOS after repeated heap doublings"
    end

    @context.eval("globalThis.echo = (...a) => a")
    hash = { "a" => 1, b: +"two", 3 => 4.5, "stamp" => Stamp.new, "after" => [+"x", :y, nil] }
    array = [1, +"two", :three, 4.5, nil, true, Stamp.new, +"after", [5]]
    result = stressed { @context.call("echo", 1, +"two", :three, hash, array, +"last") }

    assert_equal [
      1, "two", "three",
      { "3" => 4.5, "a" => 1, "b" => "two", "stamp" => Time.at(0), "after" => ["x", "y", nil] },
      [1, "two", "three", 4.5, nil, true, Time.at(0), "after", [5]],
      "last",
    ], result
  end

  # Strings the encoder decodes ahead of V8, waiting behind a Stamp in hashes
  # the runner builds and in ones V8 reads, each read from wherever its
  # String has moved to.
  def test_strings_decoded_ahead_of_v8_survive_collections_and_moves
    if RUBY_PLATFORM.include?("darwin")
      skip "GC.verify_compaction_references overflows on macOS after repeated heap doublings"
    end

    @context.eval("globalThis.echo = (...a) => a")
    texts = ["café naïve Zürich crème", "東京 大阪 名古屋 札幌",
             "\u{1F600} party \u{1F389} fun \u{1F680} go", "broken \xFF in the middle of it all",]
    few = { "stamp" => Stamp.new, "texts" => texts.map(&:dup) }
    many = { "stamp" => Stamp.new, "a" => 1, "b" => 2, "c" => 3, "d" => 4, "texts" => texts.map(&:dup) }
    built = stressed { @context.call("echo", few, [Stamp.new, *texts.map(&:dup)]) }
    read = stressed { @context.call("echo", many) }

    expected = texts.map { |text| text.scrub("�") }

    assert_equal [{ "stamp" => Time.at(0), "texts" => expected }, [Time.at(0), *expected]], built
    assert_equal [{ "stamp" => Time.at(0), "a" => 1, "b" => 2, "c" => 3, "d" => 4, "texts" => expected }], read
  end
end

# Keys and pairs the decoder keeps while it builds each hash.
class ConversionGcPairsTest < Minitest::Test
  include ConversionGc

  # Keys of objects in arrays are interned once and kept for the rest of the
  # decode, so every later row takes its keys from what the decoder holds.
  def test_keys_held_across_rows_survive_collections_and_moves
    rows = stressed do
      @context.eval(<<~JS)
        Array.from({length: 40}, (_, i) => Object.fromEntries(
          Array.from({length: 70}, (_, j) => ["key" + j, i]).concat([["caf\\u00e9", i], ["\\u6771", i]])
        ))
      JS
    end
    names = Array.new(70) { |j| "key#{j}" } + %w[café 東]

    rows.each_with_index { |row, i| assert_equal names.to_h { |name| [name, i] }, row }
    assert(rows.flat_map(&:keys).all? { |key| key.frozen? && key.equal?(-key) })
  end

  # The object's pairs have moved off the native stack by the time the Map
  # takes the object as a key, so they go in from the holder that pins them.
  def test_pairs_held_off_the_stack_go_in_before_a_key_reaches_them
    value = stressed do
      @context.eval(<<~JS)
        (() => {
          const o = Object.fromEntries(Array.from({length: 70}, (_, i) => ['k' + i, i]))
          o.m = new Map([[o, 1], [{}, 2]])
          o.z = 1
          return o
        })()
      JS
    end
    map = value["m"]

    assert_equal Array.new(70) { |i| "k#{i}" } + %w[m z], value.keys
    assert_equal Array.new(70) { |i| i }, value.values.first(70)
    assert_same value, map.keys.first
    assert_equal [{}, [1, 2], 1], [map.keys.last, map.values, value["z"]]
  end
end

# Platform flags are process-global, so this runs in a Ruby of its own. Each
# keyword value's to_s moves every movable object, among them the values not
# yet read, which only the keyword hash holds.
class ConversionGcFlagsTest < Minitest::Test
  include RubyProcess

  def test_flag_values_survive_moves_while_earlier_ones_are_read
    skip "needs CRuby's GC.verify_compaction_references" unless GC.respond_to?(:verify_compaction_references)
    code = <<~RUBY
      require "mini_racer"
      class Flag
        def initialize(text) = @text = text

        def to_s
          $read << @text
          GC.verify_compaction_references(expand_heap: true, toward: :empty)
          @text
        end
      end
      $read = []
      MiniRacer::Platform.set_flags!(**Array.new(3) { |i| [:"flag_\#{i}", Flag.new("v\#{i}")] }.to_h)
      p $read
    RUBY
    stdout, stderr, status = run_ruby(code)

    assert_predicate status, :success?, stderr.lines.first(3).join
    assert_equal %(["v0", "v1", "v2"]\n), stdout
  end
end
