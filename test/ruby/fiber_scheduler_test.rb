# frozen_string_literal: true

require "test_helper"
require "mini_racer"

begin
  require "async"
rescue LoadError
  nil
end

# Under a fiber scheduler, waiting for a context parks only the waiting fiber,
# so other fibers of the same thread keep running.
module SchedulerContexts
  # `busy` spins the runner's core; `nap` blocks the runner without using one.
  BUSY = "globalThis.busy = (ms) => { const end = Date.now() + ms; while (Date.now() < end) {} return ms };" \
         "globalThis.nap = (ms) => { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms); return ms }"

  def setup
    skip "needs CRuby and the async gem" unless RUBY_ENGINE == "ruby" && defined?(Async)
    @contexts = []
  end

  def teardown
    @contexts&.each(&:dispose)
  end

  def busy_context
    MiniRacer::Context.new.tap do |context|
      context.eval(BUSY)
      @contexts << context
    end
  end

  def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

  def ticker(task, every)
    @ticks = 0
    task.async do
      loop do
        sleep every
        @ticks += 1
      end
    end
  end
end

class FiberSchedulerTest < Minitest::Test
  include SchedulerContexts

  # The runners nap rather than spin, so they leave the CPU to the Ruby thread
  # and the ticker on any number of cores. Run one after another, the calls
  # would take count * 100 ms and the ticker would tick about once between
  # each.
  def test_contexts_run_concurrently_from_fibers_of_one_thread
    count = 4
    contexts = Array.new(count) { busy_context }
    elapsed = nil
    Sync do |task|
      ticking = ticker(task, 0.01)
      started = now
      contexts.map { |context| task.async { context.call("nap", 100) } }.each(&:wait)
      elapsed = now - started
      ticking.stop
    end

    assert_operator elapsed, :<, 0.07 * count
    assert_operator @ticks, :>=, 5
  end

  def test_fibers_sharing_a_context_take_turns
    context = busy_context
    finished = []
    elapsed = nil
    Sync do |task|
      started = now
      Array.new(2) do |i|
        task.async do
          context.call("busy", 30)
          finished << i
        end
      end.each(&:wait)
      elapsed = now - started
    end

    assert_equal [0, 1], finished.sort
    # Date.now() counts whole milliseconds, so each busy call can end early.
    assert_operator elapsed, :>=, 0.055
  end

  def test_a_callback_can_wait_on_the_scheduler_and_reenter_its_context
    context = busy_context
    result = nil
    Sync do |task|
      ticking = ticker(task, 0.005)
      context.attach("pause", proc {
        sleep 0.05
        context.eval("1 + 1")
      },)
      result = context.eval("pause() + 1")
      ticking.stop
    end

    assert_equal 3, result
    assert_operator @ticks, :>=, 3
  end
end

class FiberSchedulerLifecycleTest < Minitest::Test
  include SchedulerContexts

  def test_a_scheduler_timeout_ends_the_dispatch_and_keeps_the_context
    context = busy_context

    Sync do |task|
      assert_raises(Async::TimeoutError) do
        task.with_timeout(0.05) { context.eval("while (true) {}") }
      end
    end

    assert_equal 2, context.eval("1 + 1")
  end

  def open_descriptors = Dir.children("/dev/fd").size

  def test_dispose_closes_the_descriptors_a_fiber_wait_opened
    busy_context.dispose
    before = open_descriptors
    context = busy_context
    Sync { context.call("busy", 5) }

    assert_operator open_descriptors, :>, before
    context.dispose

    assert_equal before, open_descriptors
  end

  def test_waits_go_through_the_schedulers_io_wait
    context = busy_context
    waits = 0
    counter = Module.new do
      define_method(:io_wait) do |*args|
        waits += 1
        super(*args)
      end
    end
    Sync do
      Fiber.scheduler.singleton_class.prepend(counter)
      # A runner that answers while this thread is descheduled mid-spin, as
      # on a loaded runner, needs no wait, so a few naps make one certain.
      3.times { context.call("nap", 100) if waits.zero? }
    end

    assert_operator waits, :>=, 1
  end
end
