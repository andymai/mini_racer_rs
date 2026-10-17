# frozen_string_literal: true

require "test_helper"
require "mini_racer"

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

  # A callback that stops the context and then attaches uses up the stop, so
  # what called it runs on, as in mini_racer, and the next script runs before
  # it reports the stop.
  def attach_spender
    nest = proc do
      @context.stop
      begin
        @context.attach("g", proc { 2 })
      rescue MiniRacer::InternalError
        nil
      end
      1
    end
    stopper = proc do
      @context.stop
      1
    end
    @context.attach("nest", nest)
    @context.attach("stopper", stopper)
  end

  def test_a_leftover_microtask_runs_on_after_a_nested_attach_spends_the_stop
    attach_spender
    assert_raises(MiniRacer::ScriptTerminatedError) do
      @context.eval("Promise.resolve().then(() => { nest(); globalThis.after = 1 }); stopper(); 0")
    end
    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("globalThis.y = 1") }
    assert_equal %w[number number], @context.eval("[typeof after, typeof y]")
  end

  def test_a_getter_runs_on_after_a_nested_attach_spends_the_stop
    attach_spender

    assert_equal({ "a" => 7 }, @context.eval("({ get a() { nest(); globalThis.after = 1; return 7 } })"))
    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("globalThis.y = 1") }
    assert_equal %w[number number], @context.eval("[typeof after, typeof y]")
  end
end
