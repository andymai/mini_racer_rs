# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class ContextTest < Minitest::Test
  def setup
    @context = MiniRacer::Context.new
  end

  def test_timeout_option_terminates_a_long_script
    context = MiniRacer::Context.new(timeout: 50, marshal_stack_depth: 10)
    error = assert_raises(MiniRacer::ScriptTerminatedError) { context.eval("while (true) {}") }

    assert_equal "terminated", error.message
    assert_equal 2, context.eval("1 + 1")
  end

  def test_options_are_validated_like_upstream
    error = assert_raises(TypeError) { MiniRacer::Context.new(timeout: "1") }
    assert_equal "wrong argument type String (expected Integer)", error.message
    error = assert_raises(ArgumentError) { MiniRacer::Context.new(timeout: -1) }
    assert_equal "bad timeout", error.message
    error = assert_raises(MiniRacer::RuntimeError) { MiniRacer::Context.new(bogus: 1) }
    assert_equal "bad keyword: bogus", error.message
    assert_equal 1, MiniRacer::Context.new(snapshot: nil, verbose_exceptions: false).eval("1")
  end

  def nested(levels)
    "(() => { let a = []; for (let i = 0; i < #{levels}; i++) a = [a]; return a })()"
  end

  def test_results_nest_as_deep_as_in_mini_racer
    value = @context.eval(nested(95))
    95.times { value = value.fetch(0) }

    assert_equal [], value
    error = assert_raises(MiniRacer::RuntimeError) { @context.eval(nested(96)) }
    assert_equal "too much recursion", error.message
  end

  def test_results_nest_as_deep_inside_a_fiber
    value = Fiber.new { @context.eval(nested(95)) }.resume
    95.times { value = value.fetch(0) }

    assert_equal [], value
  end

  # The Map is decoded inside the array's dropped named properties, so it
  # hashes the array as decoded so far, which is all of its elements.
  def test_a_map_keyed_by_an_array_being_decoded_finds_it
    %w[[1,2,3] [,,1,,2]].each do |literal|
      source = "(() => { const a = #{literal}; const m = new Map([[a, 'v']]); a.m = m; return [a, m] })()"
      array, map = @context.eval(source)

      assert_equal "v", map[array], literal
      assert_equal "v", map[array.dup], literal
    end
  end

  # Each later key equals the one before it only while the objects around the
  # Map hold just their pairs decoded so far, which mini_racer has stored by
  # then, so each pair of keys is one key.
  def test_a_map_keyed_by_containers_holding_open_objects_sees_their_pairs
    source = <<~JS
      (() => {
        const outer = {x: 1, inner: {y: 2}};
        outer.inner.m = new Map([[[outer, outer.inner], "a"], [[{x: 1}, {y: 2}], "b"], [outer, "c"], [{x: 1}, "d"]]);
        return outer;
      })()
    JS
    outer = @context.eval(source)
    map = outer["inner"]["m"]

    assert_equal %w[b d], map.values
    assert_same outer, map.keys[0][0]
    assert_same outer["inner"], map.keys[0][1]
    assert_same outer, map.keys[1]
  end

  def test_only_messages_from_javascript_are_utf8
    keyword = assert_raises(MiniRacer::RuntimeError) { MiniRacer::Context.new(bogus: 1) }
    thrown = assert_raises(MiniRacer::RuntimeError) { @context.eval("throw new Error('boom')") }

    assert_equal Encoding::BINARY, keyword.message.encoding
    assert_equal Encoding::UTF_8, thrown.message.encoding
  end

  def test_eval_names_the_script_by_filename
    error = assert_raises(MiniRacer::ParseError) { @context.eval("1 +", filename: "f.js") }

    assert_equal "Uncaught SyntaxError: Unexpected end of input at f.js:1:3", error.message
    error = assert_raises(TypeError) { @context.eval("1", filename: 1) }
    assert_equal "wrong argument type Integer (expected String)", error.message
  end

  def test_utf16_source_reaches_v8_as_written
    utf16 = ->(text) { text.encode("UTF-16LE") }
    lone = utf16.call("'") + [0xD800].pack("v").force_encoding("UTF-16LE") + utf16.call("'.charCodeAt(0)")

    assert_equal 0xD800, @context.eval(lone)
    odd = (utf16.call("1").b + "\x00".b).force_encoding("UTF-16LE")
    error = assert_raises(MiniRacer::InternalError) { @context.eval(odd) }
    assert_equal "Error: Unable to deserialize cloned data.", error.message
  end

  def test_stop_terminates_the_running_script_only
    Thread.new do
      sleep 0.05
      @context.stop
    end
    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("while (true) {}") }
    assert_equal 4, @context.eval("2 + 2")
  end

  def test_dispose_is_final
    @context.dispose
    @context.dispose
    error = assert_raises(MiniRacer::ContextDisposedError) { @context.eval("1") }
    assert_equal "disposed context", error.message
    assert_raises(MiniRacer::ContextDisposedError) { @context.stop }
  end

  def test_dispose_terminates_a_script_on_another_thread
    running = Thread.new { @context.eval("while (true) {}") }
    running.report_on_exception = false
    sleep 0.05
    @context.dispose

    error = assert_raises(MiniRacer::ContextDisposedError) { running.value }
    assert_equal "disposed context", error.message
  end
end

# Deep results of each shape, on the smaller stacks of fibers and threads.
class ContextDeepResultsTest < Minitest::Test
  def setup
    @context = MiniRacer::Context.new
  end

  def teardown
    @context.dispose
  end

  NESTED = {
    "objects" => "v = {a: v}",
    "maps" => "v = new Map([['a', v]])",
    "objects in arrays" => "v = i % 2 ? [v] : {k: v}",
  }.freeze

  def depth(value)
    levels = 0
    while value.is_a?(Hash) || value.is_a?(Array)
      value = value.is_a?(Hash) ? value.values.first : value.first
      levels += 1
    end
    levels
  end

  # The decoder recurses once per level, so a deep result takes its frames
  # over and over on a fiber's or a thread's smaller stack. mini_racer 0.22.2
  # decodes each shape 95 deep in both at Ruby's default stack sizes, and
  # refuses 96.
  def test_objects_and_maps_nest_as_deep_inside_a_fiber_and_a_thread
    NESTED.each do |shape, step|
      source = ->(n) { "(() => { let v = 1; for (let i = 0; i < #{n}; i++) #{step}; return v })()" }
      fiber = Fiber.new { @context.eval(source.call(95)) }.resume
      thread = Thread.new { @context.eval(source.call(95)) }.value

      assert_equal [95, 95], [depth(fiber), depth(thread)], shape
      error = assert_raises(MiniRacer::RuntimeError) { Fiber.new { @context.eval(source.call(96)) }.resume }
      assert_equal "too much recursion", error.message
    end
  end
end
