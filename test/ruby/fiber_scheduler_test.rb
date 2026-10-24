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
  # `busy` spins the runner's core; `nap` blocks the runner without using one;
  # `span` naps and returns when the nap started and ended.
  BUSY = "globalThis.busy = (ms) => { const end = Date.now() + ms; while (Date.now() < end) {} return ms };" \
         "globalThis.nap = (ms) => { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms); return ms };" \
         "globalThis.span = (ms) => { const start = Date.now(); nap(ms); return [start, Date.now()] }"

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
  # and the ticker on any number of cores. Run one after another, each nap
  # would start after the one before it ended, and the ticker would tick about
  # once between each. The calls' wall time would tell the two apart too, but
  # a stalled CI machine adds hundreds of milliseconds to it.
  def test_contexts_run_concurrently_from_fibers_of_one_thread
    contexts = Array.new(4) { busy_context }
    spans = nil
    Sync do |task|
      ticking = ticker(task, 0.01)
      spans = contexts.map { |context| task.async { context.call("span", 100) } }.map(&:wait)
      ticking.stop
    end

    assert_operator spans.map(&:first).max, :<, spans.map(&:last).min
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

  # The ticker's next tick is due before the callback's sleep ends, so the
  # scheduler runs it first however long a stalled machine holds both up; a
  # sleep that blocked the thread would let it run only after the callback.
  def test_a_callback_can_wait_on_the_scheduler_and_reenter_its_context
    context = busy_context
    result = nil
    ticks_during_sleep = nil
    Sync do |task|
      ticking = ticker(task, 0.005)
      context.attach("pause", proc {
        before = @ticks
        sleep 0.05
        ticks_during_sleep = @ticks - before
        context.eval("1 + 1")
      },)
      result = context.eval("pause() + 1")
      ticking.stop
    end

    assert_equal 3, result
    assert_operator ticks_during_sleep, :>=, 1
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
    # Long enough that the reply cannot come before the fiber's wait opens
    # its descriptor, even on a loaded CI machine.
    Sync { context.call("nap", 100) }

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
      # A runner that answers before this thread is back on a CPU needs no
      # wait, which a loaded CI machine can cause, so up to three naps make
      # one all but certain.
      3.times { context.call("nap", 100) if waits.zero? }
    end

    assert_operator waits, :>=, 1
  end
end

# A wait under a scheduler first finds or makes the IO it waits on, and an
# interrupt that came in while the wait spun holding the GVL is raised as
# that first Ruby call returns. MINI_RACER_RS_SPIN_US makes the spin 600 ms,
# so the signal sent 150 ms in runs its trap there: on a context that made
# its IO earlier, in the check that the IO is open, and on a new one, in
# making it.
class FiberSchedulerInterruptTest < Minitest::Test
  include RubyProcess

  def test_an_interrupt_as_a_wait_begins_cancels_the_dispatch
    skip "needs CRuby, async and SIGUSR1" unless RUBY_ENGINE == "ruby" && defined?(Async) && Signal.list.key?("USR1")
    code = <<~RUBY
      require "mini_racer"
      require "async"
      Trapped = Class.new(StandardError)
      trap("USR1") { raise Trapped }
      busy = "globalThis.busy = (ms) => { const end = Date.now() + ms; while (Date.now() < end) {} return ms }"
      contexts = Array.new(2) { MiniRacer::Context.new.tap { _1.eval(busy) } }
      outcomes = Sync do
        contexts.first.call("busy", 800)
        contexts.map do |context|
          context.eval("1")
          signal = Process.spawn("sleep 0.15; kill -USR1 \#{Process.pid}")
          begin
            context.call("busy", 800)
          rescue Trapped
            :raised
          ensure
            Process.wait(signal)
          end
        end
      end
      p outcomes + contexts.map { _1.eval("1 + 1") }
    RUBY
    stdout, stderr, status = run_ruby(code, env: { "MINI_RACER_RS_SPIN_US" => "600000,0" })

    assert_predicate status, :success?, stderr
    assert_equal "[:raised, :raised, 2, 2]\n", stdout
  end
end
