# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require "timeout"

# mini_racer's stop is a one-shot V8 interrupt plus a flag the next script
# reports when it ends; these are the ways the interrupt is used up or dropped
# while the flag stays.
class SpentTerminationTest < Minitest::Test
  include RubyProcess

  def setup
    @context = MiniRacer::Context.new
  end

  def teardown
    @context&.dispose
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

  def test_a_memory_cause_while_single_threaded_lets_the_next_script_run
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      MiniRacer::Platform.set_flags! :single_threaded
      context = MiniRacer::Context.new(max_memory: 20_000_000)
      begin
        context.eval("globalThis.big = []; for (let i = 0; i < 40; i++) big.push(new Array(100000).fill(i))")
      rescue MiniRacer::V8OutOfMemoryError
        nil
      end
      context.low_memory_notification
      begin
        context.eval("globalThis.x = 1")
      rescue MiniRacer::V8OutOfMemoryError
        p [:out_of_memory, context.eval("x")]
      end
    RUBY

    assert_predicate status, :success?, err
    assert_equal "[:out_of_memory, 1]\n", out
  end

  # mini_racer replies to a terminated script with its termination cancelled,
  # and V8 runs the microtasks the script left then.
  def test_microtasks_a_terminated_script_left_run_as_it_ends
    stopper = proc do
      @context.stop
      1
    end
    @context.attach("stopper", stopper)
    assert_raises(MiniRacer::ScriptTerminatedError) do
      @context.eval("Promise.resolve().then(() => { globalThis.m = 1 }); stopper(); 0")
    end
    @context.stop
    @context.perform_microtask_checkpoint
    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("globalThis.x = 1") }
    assert_equal %w[number undefined], @context.eval("[typeof m, typeof x]")
  end

  # Deviation behavior:timeout-bounds-leftover-microtasks@ruby: mini_racer runs
  # a timed-out script's leftover microtasks with no limit and hangs here.
  def test_a_timeout_bounds_the_microtasks_a_timed_out_script_left
    context = MiniRacer::Context.new(timeout: 100)
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    assert_raises(MiniRacer::ScriptTerminatedError) do
      Timeout.timeout(5) { context.eval("Promise.resolve().then(() => { while (true) {} }); while (true) {}") }
    end
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 2
    assert_equal 2, context.eval("1 + 1")
  ensure
    context&.dispose
  end

  # The stop is consumed with the terminated script, as mini_racer's reply
  # retries and takes the cause again.
  def test_a_stop_from_a_microtask_the_reply_runs_ends_with_that_script
    raiser = proc do
      @context.stop
      raise "boom"
    end
    stopper = proc do
      @context.stop
      1
    end
    @context.attach("raiser", raiser)
    @context.attach("stopper", stopper)
    assert_raises(MiniRacer::ScriptTerminatedError) do
      @context.eval("Promise.resolve().then(() => { globalThis.ran = 1; raiser() }); stopper(); 0")
    end
    assert_equal "number", @context.eval("typeof ran")
  end

  # A timeout used up by a microtask a nested checkpoint runs lets the script
  # around it run on, as in mini_racer, which then reports the timeout.
  def test_a_timeout_spent_in_a_nested_checkpoint_lets_the_script_run_on
    context = MiniRacer::Context.new(timeout: 100)
    context.attach("nest", proc { context.perform_microtask_checkpoint })
    assert_raises(MiniRacer::ScriptTerminatedError) do
      context.eval("Promise.resolve().then(() => { while (true) {} }); nest(); globalThis.after = 1; " \
                   "const t = Date.now(); while (Date.now() - t < 300) {}; 7")
    end
    assert_equal 1, context.eval("after")
  ensure
    context&.dispose
  end
end
