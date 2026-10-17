# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class PendingTerminationTest < Minitest::Test
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
