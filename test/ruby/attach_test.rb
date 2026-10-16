# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require "timeout"

class AttachTest < Minitest::Test
  def setup
    @context = MiniRacer::Context.new
  end

  def teardown
    @context&.dispose
  end

  def test_a_callback_calls_back_into_its_context_two_levels_deep
    @context.eval("function twice(x) { return 2 * x }")
    @context.attach("inner", proc { |x| @context.call("twice", x) + 1 })
    @context.attach("outer", proc { |x| @context.eval("inner(#{x})") * 10 })

    assert_equal 70, @context.eval("outer(3)")
    assert_equal 7, @context.eval("inner(3)")
  end

  def test_an_exception_escaping_a_nested_eval_reaches_the_outer_caller
    @context.attach("inner", proc { raise KeyError, "deep" })
    @context.attach("outer", proc { @context.eval("inner()") })

    error = assert_raises(KeyError) { @context.eval("outer()") }
    assert_equal "deep", error.message
    assert_equal 2, @context.eval("1 + 1")
  end

  def test_another_thread_waits_for_the_callback_to_finish
    other = nil
    start = proc do
      other = Thread.new { @context.eval("'second'") }
      sleep 0.05
      other.alive?
    end
    @context.attach("start", start)

    assert @context.eval("start()")
    assert_equal "second", other.value
  end

  def test_a_halted_nested_eval_halts_the_script_around_it
    context = MiniRacer::Context.new(timeout: 100)
    context.attach("spin", proc { context.eval("while (true) {}") })
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)

    assert_raises(MiniRacer::ScriptTerminatedError) { context.eval("try { spin() } catch (e) {} while (true) {}") }
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 2
  ensure
    context&.dispose
  end

  def test_an_interrupt_in_a_nested_eval_abandons_only_that_eval
    guarded = proc do
      Timeout.timeout(0.05) { @context.eval("while (true) {}") }
    rescue Timeout::Error
      "ok"
    end
    @context.attach("guarded", guarded)

    assert_equal "ok!", @context.eval("guarded() + '!'")
  end

  # mini_racer keeps one raised exception per context and clears it after
  # every dispatch, nested ones included.
  def test_an_escaping_callback_error_raises_the_exception_kept_for_the_context
    @context.attach("first", proc { raise KeyError, "first" })
    @context.attach("second", proc { raise IndexError, "second" })
    @context.attach("rethrows", proc { @context.eval("throw saved") })
    @context.attach("nests", proc { @context.eval("try { second() } catch (e) { globalThis.late = e }; 1") })

    assert_raises(KeyError) { @context.eval("try { first() } catch (e) { globalThis.saved = e }; rethrows()") }
    assert_raises(MiniRacer::ContextDisposedError) { @context.eval("try { first() } catch (e) {}; nests(); throw late") }
  end
end
