# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class PendingTerminationTest < Minitest::Test
  include RubyProcess

  def setup
    @context = MiniRacer::Context.new
  end

  def teardown
    @context&.dispose
  end

  # mini_racer's attach never checks for a pending termination, so V8's
  # termination exception becomes its message.
  def test_attach_under_a_pending_stop_raises_internal_error_null
    @context.stop
    error = assert_raises(MiniRacer::InternalError) { @context.attach("f", proc { 1 }) }
    assert_equal "null", error.message
    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("1") }
    assert_raises(MiniRacer::RuntimeError) { @context.eval("f()") }
  end

  def test_attach_under_a_stop_from_its_callback_raises_internal_error_null
    nest = proc do
      @context.stop
      @context.attach("g", proc { 2 })
    end
    @context.attach("nest", nest)
    error = assert_raises(MiniRacer::InternalError) { @context.eval("nest(); 1") }
    assert_equal "null", error.message
    assert_equal 2, @context.eval("1 + 1")
  end

  # mini_racer's stop is a one-shot interrupt plus a flag the next script
  # reports when it ends. A failed attach uses up the interrupt.
  def test_after_a_failed_attach_the_next_script_runs_then_reports_the_stop
    @context.stop
    assert_raises(MiniRacer::InternalError) { @context.attach("f", proc { 1 }) }
    @context.attach("g", proc { 2 })
    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("globalThis.x = 1") }
    assert_equal 3, @context.eval("x + g()")
  end

  def test_a_stop_from_a_callback_ends_the_script_as_the_callback_returns
    stops = proc do
      @context.stop
      5
    end
    @context.attach("stops", stops)
    assert_raises(MiniRacer::ScriptTerminatedError) do
      @context.eval("globalThis.z = 0; stops(); globalThis.z = 1")
    end
    assert_equal 0, @context.eval("z")
  end

  # A single-threaded context leaves its isolate unlocked between calls, and
  # V8 drops a termination requested meanwhile; the flag stays.
  def test_a_stop_while_single_threaded_and_idle_lets_the_next_script_run
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      MiniRacer::Platform.set_flags! :single_threaded
      context = MiniRacer::Context.new
      context.stop
      begin
        context.eval("globalThis.x = 1")
      rescue MiniRacer::ScriptTerminatedError
        p [:terminated, context.eval("x")]
      end
    RUBY

    assert_predicate status, :success?, err
    assert_equal "[:terminated, 1]\n", out
  end

  def test_attach_under_a_pending_memory_cause_raises_internal_error_null
    context = MiniRacer::Context.new(max_memory: 20_000_000)
    assert_raises(MiniRacer::V8OutOfMemoryError) do
      context.eval("globalThis.big = []; for (let i = 0; i < 40; i++) big.push(new Array(100000).fill(i))")
    end
    context.low_memory_notification
    error = assert_raises(MiniRacer::InternalError) { context.attach("f", proc { 1 }) }
    assert_equal "null", error.message
    assert_raises(MiniRacer::V8OutOfMemoryError) { context.eval("big.length") }
    assert_equal 2, context.eval("big = null; 2")
  ensure
    context&.dispose
  end
end
