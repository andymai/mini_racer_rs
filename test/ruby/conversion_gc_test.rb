# frozen_string_literal: true

require_relative "test_helper"
require "mini_racer"

# Converts values full of back references while every allocation starts a
# compacting collection, so a value the decoder or encoder keeps where Ruby's
# GC cannot see it is freed or moved before it is used.
class ConversionGcTest < Minitest::Test
  # Moves every movable object while the encoder is part way through the
  # arguments.
  class Stamp
    def to_time
      GC.verify_compaction_references(expand_heap: true, toward: :empty)
      Time.at(0)
    end
  end

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

  def setup
    skip "needs CRuby's GC.stress and compaction" unless RUBY_ENGINE == "ruby"
    @context = MiniRacer::Context.new
  end

  def teardown
    @context&.dispose
  end

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
          const holey = new Array(200)
          for (let i = 0; i < 200; i++) holey[i] = "hole " + i
          return {strings, object, holey, numbers: Array.from({length: 200}, (_, i) => 2 ** 70 + i * 2 ** 20)}
        })()
      JS
    end

    assert_equal Array.new(200) { |i| "item #{i}" }, value["strings"]
    assert_equal(Array.new(200) { |i| ["item #{i}", [i, "item #{i}"]] }.to_h, value["object"])
    assert_equal Array.new(200) { |i| "hole #{i}" }, value["holey"]
    assert_equal Array.new(200) { |i| (2**70) + (i * (2**20)) }, value["numbers"]
    assert value["object"].keys.all?(&:frozen?)
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
