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
