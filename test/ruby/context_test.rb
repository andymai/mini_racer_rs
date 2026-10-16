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
    assert_raises(NotImplementedError) { MiniRacer::Context.new(max_memory: 1) }
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

  def test_low_memory_notification_returns_nil_and_leaves_a_stop_pending
    @context.eval("var big = 'x'.repeat(1e6)")

    assert_nil @context.low_memory_notification
    @context.stop
    @context.low_memory_notification
    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("1") }
    @context.dispose
    assert_raises(MiniRacer::ContextDisposedError) { @context.low_memory_notification }
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

    assert_raises(MiniRacer::ScriptTerminatedError) { running.value }
  end
end
