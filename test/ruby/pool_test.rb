# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require "timeout"

begin
  require "async"
rescue LoadError
  nil
end

module PoolHelpers
  # `nap` blocks the runner without using a core; `span` naps and returns when
  # the nap started and ended; `hog` runs out of any max_memory: it is given.
  SCRIPT = "globalThis.nap = (ms) => { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms); return ms };" \
           "globalThis.span = (ms) => { const start = Date.now(); nap(ms); return [start, Date.now()] };" \
           "globalThis.hog = () => { const a = []; for (;;) a.push(new Array(100000).fill(1)) }"

  def setup
    @pools = []
  end

  def teardown
    @pools.each(&:shutdown)
  end

  def pool(size: 1, **options, &setup)
    MiniRacerRs::Pool.new(size:, **options, &setup).tap { @pools << _1 }
  end

  # Contexts the pool builds, in order, each loaded with SCRIPT.
  def recording_pool(built, size: 1, **options)
    pool(size:, **options) do |context|
      context.eval(SCRIPT)
      built << context
    end
  end

  def disposed?(context)
    context.eval("1")
    false
  rescue MiniRacer::ContextDisposedError
    true
  end

  # Replacements are built, and retired contexts disposed, on the pool's own
  # threads, so the test waits for them.
  def eventually(within: 10)
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + within
    until yield
      flunk "timed out waiting" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
      sleep 0.01
    end
  end

  # A thread that checks a context out and holds it until `release` is called.
  def hold(pool)
    held = Queue.new
    release = Queue.new
    thread = Thread.new do
      pool.with do |context|
        held << context
        release.pop
      end
    end
    [held.pop, lambda {
      release << true
      thread.join
    },]
  end
end

class PoolSizingTest < Minitest::Test
  include PoolHelpers

  def test_new_builds_every_context_and_runs_the_setup_block_for_each
    built = []
    recording_pool(built, size: 3)

    assert_equal 3, built.size
    assert_equal 3, built.map(&:object_id).uniq.size
    built.each { |context| assert_equal 5, context.eval("nap(5)") }
  end

  def test_with_returns_the_value_of_its_block
    assert_equal(42, pool.with { |context| context.eval("40 + 2") })
  end

  def test_a_pool_without_a_setup_block_builds_bare_contexts
    assert_equal("undefined", pool(size: 2).with { |context| context.eval("typeof nap") })
  end

  def test_size_is_one_to_1024_and_max_uses_positive_or_nil
    assert_raises_message(ArgumentError, "bad size") { MiniRacerRs::Pool.new(size: 0) }
    assert_raises_message(ArgumentError, "bad size") { MiniRacerRs::Pool.new(size: -1) }
    assert_raises_message(ArgumentError, "bad size") { MiniRacerRs::Pool.new(size: 1025) }
    assert_raises_message(ArgumentError, "bad size") { MiniRacerRs::Pool.new(size: 2**64) }
    assert_raises_message(ArgumentError, "bad max_uses") { MiniRacerRs::Pool.new(size: 1, max_uses: 0) }
    assert_raises_message(ArgumentError, "bad max_uses") { MiniRacerRs::Pool.new(size: 1, max_uses: -(2**64)) }
    assert_equal(1, pool(max_uses: nil).with { 1 })
    assert_equal(1, pool(max_uses: 2**62).with { 1 })
  end

  def test_size_and_max_uses_take_integers_as_context_new_does
    assert_raises_message(TypeError, "wrong argument type String (expected Integer)") do
      MiniRacerRs::Pool.new(size: "1")
    end
    assert_raises_message(TypeError, "wrong argument type nil (expected Integer)") do
      MiniRacerRs::Pool.new(size: nil)
    end
    assert_raises_message(TypeError, "wrong argument type Float (expected Integer)") do
      MiniRacerRs::Pool.new(size: 1, max_uses: 1.0)
    end
    assert_raises_message(TypeError, "wrong argument type false (expected Integer)") do
      MiniRacerRs::Pool.new(size: 1, max_uses: false)
    end
  end

  def test_size_is_a_required_keyword
    assert_raises_message(ArgumentError, "missing keyword: :size") { MiniRacerRs::Pool.new }
    assert_raises(ArgumentError) { MiniRacerRs::Pool.new(1) }
  end

  def test_other_keywords_go_to_context_new_unchanged
    assert_raises_message(MiniRacer::RuntimeError, "bad keyword: bogus") do
      MiniRacerRs::Pool.new(size: 1, bogus: 1)
    end
    assert_raises_message(ArgumentError, "bad timeout") { MiniRacerRs::Pool.new(size: 1, timeout: -1) }

    timed = pool(timeout: 50)

    assert_raises(MiniRacer::ScriptTerminatedError) { timed.with { |context| context.eval("for (;;) {}") } }
  end

  def test_a_setup_error_raises_from_new_after_disposing_the_contexts_built
    built = []
    error = assert_raises(RuntimeError) do
      MiniRacerRs::Pool.new(size: 3) do |context|
        built << context
        raise "setup failed" if built.size == 2
      end
    end

    assert_equal "setup failed", error.message
    assert_equal 2, built.size
    assert(built.all? { disposed?(_1) })
  end

  def test_with_needs_a_block
    assert_raises_message(LocalJumpError, "no block given (yield)") { pool.with }
  end

  def test_an_uninitialized_pool_refuses_checkouts
    assert_raises(MiniRacer::InternalError) { MiniRacerRs::Pool.allocate.with { 1 } }
    assert_raises(MiniRacer::InternalError) { MiniRacerRs::Pool.allocate.shutdown }
  end

  def test_a_pool_cannot_be_initialized_twice
    made = pool
    assert_raises(MiniRacer::InternalError) { made.send(:initialize, size: 1) }
  end

  private

  def assert_raises_message(klass, message, &)
    error = assert_raises(klass, &)

    assert_equal message, error.message
  end
end

class PoolReplacementTest < Minitest::Test
  include PoolHelpers

  def test_checkouts_take_turns_on_the_contexts
    built = []
    contexts = recording_pool(built)

    3.times { assert_same(built.first, contexts.with { _1 }) }
  end

  def test_max_uses_retires_a_context_after_that_many_checkouts
    built = []
    contexts = recording_pool(built, max_uses: 2)
    first = built.first

    2.times { assert_same(first, contexts.with { _1 }) }
    replacement = contexts.with { _1 }

    refute_same first, replacement
    assert_same built.last, replacement
    eventually { disposed?(first) }
  end

  def test_running_out_of_memory_retires_the_context_and_still_raises
    built = []
    contexts = recording_pool(built, max_memory: 20_000_000)
    first = built.first

    error = assert_raises(MiniRacer::V8OutOfMemoryError) { contexts.with { _1.eval("hog()") } }
    assert_equal "out of memory", error.message
    refute_same(first, contexts.with { _1 })
    eventually { disposed?(first) }
  end

  def test_a_context_disposed_in_its_checkout_is_retired
    built = []
    contexts = recording_pool(built)

    assert_raises(MiniRacer::ContextDisposedError) do
      contexts.with do |context|
        context.dispose
        context.eval("1")
      end
    end
    second = contexts.with { _1 }

    refute_same built.first, second
    contexts.with(&:dispose)

    refute_same(second, contexts.with { _1 })
    assert_equal 3, built.size
  end

  def test_a_terminated_script_and_other_errors_keep_the_context
    built = []
    contexts = recording_pool(built, timeout: 50)

    assert_raises(MiniRacer::ScriptTerminatedError) { contexts.with { _1.eval("for (;;) {}") } }
    assert_raises(MiniRacer::RuntimeError) { contexts.with { _1.eval("throw new Error('no')") } }
    assert_raises(MiniRacer::ParseError) { contexts.with { _1.eval("}") } }
    assert_raises(ZeroDivisionError) { contexts.with { 1 / 0 } }

    assert_same(built.first, contexts.with { _1 })
    assert_equal 1, built.size
  end

  def test_a_failed_replacement_raises_from_the_next_checkout_and_is_built_again
    calls = 0
    contexts = pool(max_uses: 1) do
      calls += 1
      raise ArgumentError, "build #{calls} failed" if calls == 2
    end

    contexts.with { 1 }
    eventually { calls == 2 }
    error = assert_raises(ArgumentError) { contexts.with { 1 } }

    assert_equal "build 2 failed", error.message
    assert_equal(2, contexts.with { 2 })
    assert_equal 3, calls
  end
end

class PoolCheckoutTest < Minitest::Test
  include PoolHelpers

  def test_break_throw_and_thread_kill_check_the_context_back_in
    built = []
    contexts = recording_pool(built)

    assert_equal(5, contexts.with { break 5 })
    assert_equal 6, catch(:done) { contexts.with { throw :done, 6 } }
    inside = Queue.new
    thread = Thread.new do
      contexts.with do
        inside << true
        sleep
      end
    end
    inside.pop
    sleep 0.01 until thread.stop?
    thread.kill.join

    assert_same(built.first, contexts.with { _1 })
    assert_equal 1, built.size
  end

  # The checkout left by break wakes no one, so the waiting one finds the
  # context when it looks again.
  def test_a_context_left_by_break_reaches_a_waiting_checkout
    contexts = pool
    waiter = nil
    held = contexts.with do |context|
      waiter = Thread.new { contexts.with { _1 } }
      sleep 0.01 until waiter.status == "sleep"
      break context
    end

    assert_same held, waiter.value
  end

  def test_a_timeout_while_waiting_leaves_the_pool_usable
    contexts = pool
    held, release = hold(contexts)

    assert_raises(Timeout::Error) { Timeout.timeout(0.05) { contexts.with { 1 } } }
    release.call

    assert_same(held, contexts.with { _1 })
  end

  def test_a_waiting_checkout_gets_the_context_checked_in
    contexts = pool
    held, release = hold(contexts)
    waiter = Thread.new { contexts.with { _1 } }
    sleep 0.01 until waiter.status == "sleep"
    release.call

    assert_same held, waiter.value
  end

  def test_threads_render_on_separate_contexts_in_parallel
    contexts = pool(size: 4) { _1.eval(SCRIPT) }
    spans = Array.new(4) { Thread.new { contexts.with { _1.call("span", 300) } } }.map(&:value)

    assert_operator spans.map(&:first).max, :<, spans.map(&:last).min
  end
end

class PoolTurnsTest < Minitest::Test
  include PoolHelpers
  include RubyProcess

  # A thread that checks its context in and at once asks again would take it
  # back before a woken waiter ran, so a freed context goes to the checkout
  # that has waited longest, and a new one waits behind those waiting. The
  # threads all wait before the first context is freed: a thread alone with
  # the GVL checks out with no one waiting until its time slice ends.
  def test_waiting_checkouts_take_turns
    contexts = pool
    %i[block break].each do |ending|
      stop = false
      counts = Array.new(4, 0)
      threads = contexts.with do
        started = Array.new(4) do |index|
          Thread.new do
            until stop
              if ending == :break
                contexts.with { |context| break context.eval("1") }
              else
                contexts.with { |context| context.eval("1") }
              end
              counts[index] += 1
            end
          end
        end
        sleep 0.01 until started.all? { _1.status == "sleep" }
        started
      end
      sleep 0.5
      stop = true
      threads.each(&:join)

      assert_operator counts.min, :>=, counts.max / 2, "#{ending}: #{counts.inspect}"
    end
  end

  # A signal trap that checks out while its own thread waits in a checkout
  # takes that wait's place in line; behind it, it would wait for a wait that
  # cannot go on until the trap returns.
  def test_a_trap_checks_out_while_its_thread_waits
    skip "needs CRuby and SIGUSR1" unless RUBY_ENGINE == "ruby" && Signal.list.key?("USR1")
    code = <<~RUBY
      require "mini_racer"
      pool = MiniRacerRs::Pool.new(size: 1)
      trapped = nil
      trap("USR1") { trapped = pool.with { _1.eval("'trap'") } }
      held = Queue.new
      release = Queue.new
      Thread.new { pool.with { held << true; release.pop } }
      held.pop
      Thread.new do
        sleep 0.2 until Thread.main.status == "sleep"
        Process.kill("USR1", Process.pid)
        sleep 0.2
        release << true
        sleep 5
        warn "stuck: trapped \#{trapped.inspect}"
        exit!(2)
      end
      puts pool.with { _1.eval("'main'") }, trapped
    RUBY
    stdout, stderr, status = run_ruby(code)

    assert_predicate status, :success?, stderr
    assert_equal "main\ntrap\n", stdout
  end

  def test_contexts_stay_put_through_compaction
    skip "needs GC.compact" unless GC.respond_to?(:compact)
    built = []
    contexts = recording_pool(built, size: 2, max_uses: 3)
    10.times do |i|
      GC.compact

      assert_equal(i, contexts.with { _1.eval(i.to_s) })
    end
  end

  # The pool is held only by a class, never by a local that would pin it, so
  # compaction moves it before the thread that builds each replacement runs.
  def test_a_pool_moved_by_compaction_before_its_thread_runs
    skip "needs CRuby's GC.verify_compaction_references" unless GC.respond_to?(:verify_compaction_references)
    code = <<~RUBY
      require "mini_racer"
      class Holder
        class << self
          attr_reader :pool

          def make
            @pool = MiniRacerRs::Pool.new(size: 1, max_uses: 1) { _1.eval("var n = 41") }
            nil
          end

          def use = pool.with { _1.eval("n + 1") }
        end
      end
      Holder.make
      5.times do
        Holder.use
        GC.verify_compaction_references(expand_heap: true, toward: :empty)
      end
      puts Holder.use
      Holder.pool.shutdown
    RUBY
    stdout, stderr, status = run_ruby(code)

    assert_predicate status, :success?, stderr
    assert_equal "42\n", stdout
  end
end

class PoolNestingTest < Minitest::Test
  include PoolHelpers

  def test_a_nested_checkout_on_one_fiber_reuses_its_context
    contexts = pool

    assert(contexts.with { |outer| contexts.with { |inner| inner.equal?(outer) } })
  end

  def test_a_nested_checkout_is_not_a_use
    built = []
    contexts = recording_pool(built, max_uses: 2)
    first = built.first

    contexts.with { contexts.with { contexts.with { 1 } } }

    assert_same(first, contexts.with { _1 })
    refute_same(first, contexts.with { _1 })
  end

  def test_a_spent_context_raised_through_a_nested_checkout_is_retired
    built = []
    contexts = recording_pool(built, max_memory: 20_000_000)
    first = built.first

    contexts.with do
      contexts.with { _1.eval("hog()") }
    rescue MiniRacer::V8OutOfMemoryError
      nil
    end

    refute_same(first, contexts.with { _1 })
  end

  def test_another_fiber_takes_another_context
    contexts = pool(size: 2)

    refute(contexts.with { |outer| Fiber.new { contexts.with { |inner| inner.equal?(outer) } }.resume })
  end

  def test_a_callback_that_reenters_the_pool_runs_on_its_own_context
    contexts = pool

    same = contexts.with do |context|
      context.attach("again", proc { contexts.with { _1.equal?(context) && _1.eval("40 + 2") } })
      context.eval("again()")
    end

    assert_equal 42, same
  end
end

class PoolShutdownTest < Minitest::Test
  include PoolHelpers

  def test_shutdown_disposes_idle_contexts_now
    built = []
    contexts = recording_pool(built, size: 3)
    contexts.shutdown

    assert(built.all? { disposed?(_1) })
  end

  def test_a_context_in_use_is_disposed_when_its_checkout_ends
    contexts = pool
    leaked = contexts.with do |context|
      contexts.shutdown

      assert_equal 2, context.eval("1 + 1")
      context
    end

    assert disposed?(leaked)
  end

  # A checkout left by break, throw or Thread#kill makes no Ruby call on its
  # way out, so a later shutdown disposes what it left.
  def test_shutdown_again_disposes_a_context_left_by_break
    contexts = pool
    leaked = contexts.with do |context|
      contexts.shutdown
      break context
    end
    contexts.shutdown

    assert disposed?(leaked)
  end

  def test_checkouts_after_shutdown_raise
    contexts = pool
    contexts.shutdown
    contexts.shutdown
    error = assert_raises(MiniRacerRs::Pool::ShutdownError) { contexts.with { 1 } }

    assert_equal "the context pool is shut down", error.message
    assert_kind_of MiniRacer::Error, error
    assert_raises(MiniRacerRs::Pool::ShutdownError) { contexts.with { contexts.with { 1 } } }
  end

  def test_shutdown_wakes_waiting_checkouts
    contexts = pool
    _, release = hold(contexts)
    waiter = Thread.new { contexts.with { 1 } }
    waiter.report_on_exception = false
    sleep 0.01 until waiter.status == "sleep"
    contexts.shutdown

    assert_raises(MiniRacerRs::Pool::ShutdownError) { waiter.join }
    release.call
  end

  def test_a_context_being_built_at_shutdown_is_disposed_once_built
    built = []
    gate = Queue.new
    contexts = pool(max_uses: 1) do |context|
      built << context
      gate.pop if built.size == 2
    end
    contexts.with { 1 }
    eventually { built.size == 2 }
    contexts.shutdown
    gate << true

    eventually { disposed?(built.last) }
  end
end

class PoolFiberSchedulerTest < Minitest::Test
  include PoolHelpers

  def setup
    skip "needs CRuby and the async gem" unless RUBY_ENGINE == "ruby" && defined?(Async)
    super
  end

  # The waiting fiber parks, so the ticker keeps ticking while it waits.
  def test_a_checkout_waiting_under_a_scheduler_parks_only_its_fiber
    contexts = pool
    ticks = 0
    order = []
    Sync do |task|
      ticker = task.async do
        loop do
          sleep 0.005
          ticks += 1
        end
      end
      holder = task.async do
        contexts.with do
          order << :held
          sleep 0.1
          order << :released
        end
      end
      waiter = task.async do
        before = ticks
        contexts.with { order << :taken }
        ticks - before
      end
      holder.wait

      assert_operator waiter.wait, :>=, 3
      ticker.stop
    end

    assert_equal %i[held released taken], order
  end

  def test_a_scheduler_timeout_cancels_a_waiting_checkout
    contexts = pool
    Sync do |task|
      holder = task.async { contexts.with { sleep 0.1 } }
      assert_raises(Async::TimeoutError) do
        task.with_timeout(0.02) { contexts.with { 1 } }
      end
      holder.wait

      assert_equal(2, contexts.with { _1.eval("1 + 1") })
    end
  end

  def test_fibers_render_on_separate_contexts_at_once
    contexts = pool(size: 3) { _1.eval(SCRIPT) }
    spans = Sync do |task|
      Array.new(3) { task.async { contexts.with { _1.call("span", 200) } } }.map(&:wait)
    end

    assert_operator spans.map(&:first).max, :<, spans.map(&:last).min
  end
end
