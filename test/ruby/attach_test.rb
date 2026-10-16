# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require "timeout"

class AttachTest < Minitest::Test
  include RubyProcess

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

  # mini_racer raises the interrupt before it clears the kept exception.
  def test_an_interrupted_dispatch_keeps_the_callback_exception
    @context.attach("raises", proc { raise KeyError, "kept" })
    assert_raises(Timeout::Error) do
      Timeout.timeout(0.2) { @context.eval("try { raises() } catch (e) { globalThis.saved = e }; while (true) {}") }
    end

    assert_raises(KeyError) { @context.eval("throw saved") }
  end

  def test_threads_attaching_while_the_context_is_busy_get_their_own_ids
    release = Queue.new
    @context.attach("hold", proc { release.pop })
    holder = Thread.new { @context.eval("hold()") }
    sleep 0.05
    names = %w[a b c d]
    attachers = names.map { |name| Thread.new { @context.attach(name, proc { name }) } }
    sleep 0.05
    release << 0
    [holder, *attachers].each(&:join)

    assert_equal(names, names.map { |name| @context.eval("#{name}()") })
  end

  def test_attach_on_a_disposed_context_raises_and_keeps_nothing
    context = MiniRacer::Context.new
    context.dispose

    assert_raises(MiniRacer::ContextDisposedError) { context.attach("late", proc { 1 }) }
  end

  def test_dispose_inside_a_callback_is_refused_on_any_fiber
    @context.attach("disposes", proc { Enumerator.new { |y| y << @context.dispose }.next })

    error = assert_raises(MiniRacer::RuntimeError) { @context.eval("disposes()") }
    assert_match(/\Acontext dispose: /, error.message)
    assert_equal 2, @context.eval("1 + 1")
  end

  def test_deep_nesting_warns_as_mini_racer_does
    _, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      context = MiniRacer::Context.new
      context.attach("down", proc { |n| n.zero? ? 0 : context.eval("down(\#{n - 1}) + 1") })
      exit(context.eval("down(60)") == 60 ? 0 : 1)
    RUBY

    assert_predicate status, :success?, err
    assert_equal "mini_racer: deep js->ruby->js recursion, depth=50\n", err
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
