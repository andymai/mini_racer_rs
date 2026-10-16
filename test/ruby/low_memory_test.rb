# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class LowMemoryTest < Minitest::Test
  def setup
    @context = MiniRacer::Context.new
  end

  def test_low_memory_notification_collects_before_it_returns
    @context.eval("globalThis.ref = (() => new WeakRef({payload: 'x'.repeat(1000)}))()")

    assert_nil @context.low_memory_notification
    assert @context.eval("ref.deref() === undefined")
  end

  def test_low_memory_notification_leaves_a_stop_pending
    @context.stop
    @context.low_memory_notification

    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("1") }
    @context.dispose
    assert_raises(MiniRacer::ContextDisposedError) { @context.low_memory_notification }
  end

  def test_low_memory_notification_from_a_callback_keeps_its_exception
    @context.attach("raiser", proc { raise ArgumentError, "boom" })
    @context.attach("gc", proc { @context.low_memory_notification })

    assert_equal 7, @context.eval("(() => { gc(); return 7 })()")
    error = assert_raises(ArgumentError) { @context.eval("try { raiser() } catch (e) { gc(); throw e }") }
    assert_equal "boom", error.message
  end
end
