# frozen_string_literal: true

require_relative "test_helper"
require "mini_racer"

# Decodes a value full of back references while every allocation starts a
# compacting collection, so a value the decoder keeps where Ruby's GC cannot
# see it is freed or moved before it is used.
class DecodeGcTest < Minitest::Test
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

  private

  def stressed
    GC.auto_compact = true
    GC.stress = true
    yield
  ensure
    GC.stress = false
    GC.auto_compact = false
    GC.verify_compaction_references(expand_heap: true, toward: :empty)
  end
end
