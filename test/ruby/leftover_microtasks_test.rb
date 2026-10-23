# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require "timeout"

class LeftoverMicrotasksTest < Minitest::Test
  HOG = "(function () { const a = []; while (true) a.push(new Array(100000).fill(1)) })()"

  def setup
    @context = MiniRacer::Context.new
    attach_stoppers(@context)
  end

  def teardown
    @context&.dispose
  end

  def attach_stoppers(context)
    stopper = proc do
      context.stop
      1
    end
    raiser = proc do
      context.stop
      raise "boom"
    end
    context.attach("stopper", stopper)
    context.attach("raiser", raiser)
  end

  # mini_racer replies to a terminated script with its termination cancelled,
  # and V8 runs the microtasks the script left then.
  def test_microtasks_a_terminated_script_left_run_as_it_ends
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
    assert_raises(MiniRacer::ScriptTerminatedError) do
      @context.eval("Promise.resolve().then(() => { globalThis.ran = 1; raiser() }); stopper(); 0")
    end
    assert_equal "number", @context.eval("typeof ran")
  end

  # A stop V8 acts on in a leftover microtask is carried, spent: the next
  # script runs, then reports it.
  def test_a_stop_acted_on_in_leftover_microtasks_carries_to_the_next_script
    assert_raises(MiniRacer::ScriptTerminatedError) do
      @context.eval("Promise.resolve().then(() => { stopper() }); stopper(); 0")
    end
    assert_raises(MiniRacer::ScriptTerminatedError) { @context.eval("globalThis.x = 1") }
    assert_equal 1, @context.eval("x")
  end

  def test_a_memory_cause_in_leftover_microtasks_carries_to_the_next_script
    context = MiniRacer::Context.new(max_memory: 20_000_000)
    attach_stoppers(context)
    assert_raises(MiniRacer::ScriptTerminatedError) do
      context.eval("Promise.resolve().then(() => { globalThis.started = 1; #{HOG} }); stopper(); 0")
    end
    assert_raises(MiniRacer::V8OutOfMemoryError) { context.eval("typeof started") }
    assert_equal 2, context.eval("1 + 1")
  ensure
    context&.dispose
  end

  # A Ruby interrupt abandons the eval, and its leftover microtasks still run:
  # their callbacks only throw, as mini_racer answers them.
  def test_callbacks_from_leftover_microtasks_of_an_abandoned_eval_only_throw
    calls = []
    @context.attach("log", proc { |x| calls << x })
    assert_raises(Timeout::Error) do
      Timeout.timeout(0.3) do
        @context.eval("Promise.resolve().then(() => { try { log('drained') } " \
                      "catch (e) { globalThis.caught = String(e.message) }; globalThis.after = 1 }); while (true) {}")
      end
    end
    assert_empty calls
    assert_equal %w[string number terminated], @context.eval("[typeof caught, typeof after, caught]")
  end

  # Deviation behavior:timeout-bounds-leftover-microtasks@ruby: a nested call
  # that uses up the termination does not lift it while the leftover
  # microtasks run, so the timeout still ends them; mini_racer hangs here.
  def test_a_nested_call_in_leftover_microtasks_keeps_the_bound
    context = MiniRacer::Context.new(timeout: 100)
    context.eval("Object.defineProperty(globalThis, 'o', { get() { while (true) {} } })")
    nest = proc do
      context.attach("o.f", proc { 1 })
    rescue MiniRacer::InternalError
      nil
    end
    context.attach("nest", nest)
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    assert_raises(MiniRacer::ScriptTerminatedError) do
      Timeout.timeout(5) do
        context.eval("Promise.resolve().then(() => { nest(); while (true) {} }); while (true) {}")
      end
    end
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 2
  ensure
    context&.dispose
  end
end

# The heap limit outranks a stop or a timeout and ends with its dispatch, so
# a hit in a script's leftover microtasks, in a checkpoint or as a result
# serializes is that dispatch's to report, and the next script runs.
class LeftoverMicrotasksHeapLimitTest < Minitest::Test
  include RubyProcess

  # Outcomes, one per line, of `code` run in a process of its own with a heap
  # limit of 64 MiB, since a hit past V8's room would abort the process.
  def under_heap_limit(code)
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      MiniRacerRs.heap_limit = 64 << 20
      HOG = #{LeftoverMicrotasksTest::HOG.dump}
      def outcome
        "ok \#{yield.inspect}"
      rescue => e
        "\#{e.class}: \#{e.message}"
      end
      #{code}
    RUBY
    assert_predicate status, :success?, "stdout:\n#{out}\nstderr:\n#{err}"
    out.lines(chomp: true)
  end

  REACHED = "MiniRacerRs::HeapLimitError: heap limit of #{64 << 20} bytes reached".freeze

  # The heap limit outranks a stop or a timeout and ends with its dispatch:
  # leftover microtasks that reach it report it with the script that left
  # them, and the next script runs.
  def test_a_heap_limit_in_leftover_microtasks_ends_with_the_script_that_left_them
    lines = under_heap_limit(<<~RUBY)
      context = MiniRacer::Context.new
      context.attach("stopper", proc { context.stop; 1 })
      puts outcome { context.eval("Promise.resolve().then(() => { \#{HOG} }); stopper(); 0") }
      puts outcome { context.eval("1 + 1") }
      timed = MiniRacer::Context.new(timeout: 200)
      puts outcome { timed.eval("Promise.resolve().then(() => { \#{HOG} }); while (true) {}") }
      puts outcome { timed.eval("1 + 1") }
    RUBY

    assert_equal [REACHED, "ok 2", REACHED, "ok 2"], lines
  end

  # A checkpoint reports no termination, so one whose microtasks reach the
  # limit raises it itself; the script around it ends with it too.
  def test_a_heap_limit_in_a_microtask_checkpoint_raises_from_it
    lines = under_heap_limit(<<~RUBY)
      context = MiniRacer::Context.new
      nested = nil
      context.attach("checkpoint", proc { nested = outcome { context.perform_microtask_checkpoint }; 1 })
      puts outcome { context.eval("Promise.resolve().then(() => { \#{HOG} }); checkpoint(); 0") }
      puts nested
      puts outcome { context.eval("1 + 1") }
    RUBY

    assert_equal [REACHED, REACHED, "ok 2"], lines
  end

  # A getter runs as the result serializes, after the script's verdict.
  def test_a_heap_limit_as_a_result_serializes_ends_with_that_script
    lines = under_heap_limit(<<~RUBY)
      context = MiniRacer::Context.new
      puts outcome { context.eval("({ get x() { \#{HOG} } })") }
      puts outcome { context.eval("1 + 1") }
      puts outcome { context.eval("({ get x() { return new Array(7e6).fill(1.5).length } })") }
      puts outcome { context.eval("1 + 1") }
    RUBY

    assert_equal [REACHED, "ok 2", 'ok {"x" => 7000000}', "ok 2"], lines
  end
end
