# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require "timeout"

# MiniRacerRs.counters reads a context's counts from atomics: no dispatch, no
# context lock. mini_racer 0.22.1 has no counterpart.
module CounterHelpers
  NONE = { stop: 0, timeout: 0, interrupt: 0, max_memory: 0, heap_exhausted: 0 }.freeze
  COUNTED = %i[dispatches callbacks bytes_to_js bytes_from_js].freeze

  def counters(context) = MiniRacerRs.counters(context)

  def now = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)

  # Waits until the context has been sent `count` dispatches, so a thread is
  # interrupted or read only once its script runs, however loaded the machine.
  def dispatched(context, count)
    Timeout.timeout(10) { sleep 0.001 until counters(context)[:dispatches] >= count }
  end

  # The change in each count a block makes.
  def counted(context, keys = COUNTED)
    before = counters(context)
    yield
    after = counters(context)
    keys.to_h { |key| [key, after[key] - before[key]] }
  end
end

class CountersTest < Minitest::Test
  include RubyProcess
  include CounterHelpers

  class Halted < StandardError
  end

  # A result's bytes are V8's: a 2-byte header, then 'abc' as a tag, a length
  # and its 3 bytes, 2 as a tag and a varint, undefined as a tag. A call's
  # request is the array ["id", arg] in the same format.
  def test_counts_each_dispatch_and_its_bytes_each_way
    context = MiniRacer::Context.new

    assert_equal({ dispatches: 1, callbacks: 0, bytes_to_js: 5, bytes_from_js: 7 },
                 counted(context) { context.eval("'abc'") },)
    assert_equal({ dispatches: 1, callbacks: 0, bytes_to_js: 27, bytes_from_js: 3 },
                 counted(context) { context.eval("function id(x) { return x }") },)
    assert_equal({ dispatches: 1, callbacks: 0, bytes_to_js: 16, bytes_from_js: 7 },
                 counted(context) { context.call("id", "abc") },)
    assert_equal({ dispatches: 1, callbacks: 0, bytes_to_js: 4, bytes_from_js: 5 },
                 counted(context) { context.eval("'é'") },)
    assert_equal({ dispatches: 1, callbacks: 0, bytes_to_js: 0, bytes_from_js: 1 },
                 counted(context) { context.pump_message_loop },)
    %i[perform_microtask_checkpoint low_memory_notification].each do |operation|
      assert_equal({ dispatches: 1, callbacks: 0, bytes_to_js: 0, bytes_from_js: 0 },
                   counted(context) { context.public_send(operation) },)
    end
    assert_equal 1, counted(context) { context.heap_stats }[:dispatches]
    assert_equal NONE, counters(context)[:terminated]
  end

  # In: the attach's ["ruby_eval", 0], both sources and the callback's 42.
  # Out: the callback's arguments with its id, and both results.
  def test_counts_callbacks_and_the_dispatches_they_nest
    context = MiniRacer::Context.new
    attached = counted(context) { context.attach("ruby_eval", proc { |source| context.eval(source) }) }
    nested = counted(context) { assert_equal 43, context.eval("ruby_eval('6 * 7') + 1") }

    assert_equal({ dispatches: 1, callbacks: 0, bytes_to_js: 20, bytes_from_js: 0 }, attached)
    assert_equal({ dispatches: 2, callbacks: 1, bytes_to_js: 22 + 5 + 4, bytes_from_js: 20 + 4 + 4 }, nested)
  end

  def test_counts_terminations_by_cause
    stopped = MiniRacer::Context.new
    stopped.stop
    assert_raises(MiniRacer::ScriptTerminatedError) { stopped.eval("1") }
    timed = MiniRacer::Context.new(timeout: 50)
    assert_raises(MiniRacer::ScriptTerminatedError) { timed.eval("while (true) {}") }
    interrupted = MiniRacer::Context.new
    assert_raises(Timeout::Error) { Timeout.timeout(0.3) { interrupted.eval("while (true) {}") } }
    capped = MiniRacer::Context.new(max_memory: 50_000_000)
    assert_raises(MiniRacer::V8OutOfMemoryError) do
      capped.eval("let a = []; while (true) { a.push(new Array(10000).fill(0)) }")
    end

    assert_equal NONE.merge(stop: 1), counters(stopped)[:terminated]
    assert_equal NONE.merge(timeout: 1), counters(timed)[:terminated]
    assert_equal NONE.merge(interrupt: 1), counters(interrupted)[:terminated]
    assert_equal NONE.merge(max_memory: 1), counters(capped)[:terminated]
  end

  def test_counts_a_thread_raise_and_a_throw_out_of_a_callback_as_interrupts
    context = MiniRacer::Context.new
    context.attach("leave", proc { throw :out })
    catch(:out) { context.eval("leave(); while (true) {}") }
    thread = Thread.new { context.eval("while (true) {}") }
    thread.report_on_exception = false
    dispatched(context, 3)
    thread.raise(Halted)
    assert_raises(Halted) { thread.join }

    assert_equal NONE.merge(interrupt: 2), counters(context)[:terminated]
    assert_equal 1, counters(context)[:callbacks]
  end

  def test_counts_javascript_time_but_not_a_callbacks
    context = MiniRacer::Context.new
    context.attach("nap", proc { sleep 0.3 })
    started = now
    busy = counted(context, [:js_ns]) do
      context.eval("const until = Date.now() + 50; while (Date.now() < until) {}")
    end[:js_ns]
    wall = now - started

    assert_operator busy, :>=, 49_000_000
    assert_operator busy, :<=, wall
    assert_operator counted(context, [:js_ns]) { context.eval("nap()") }[:js_ns], :<, 100_000_000
  end

  def test_counts_collections_and_their_time
    context = MiniRacer::Context.new
    context.eval("globalThis.junk = []; for (let i = 0; i < 1e5; i++) junk.push({ i }); junk = null")
    collected = counted(context, %i[gc_count gc_ns js_ns]) { context.low_memory_notification }

    assert_operator collected[:gc_count], :>=, 1
    assert_operator collected[:gc_ns], :>, 0
    assert_equal 0, collected[:js_ns]
  end

  def test_counts_each_collection_once
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      MiniRacer::Platform.set_flags!(:expose_gc)
      context = MiniRacer::Context.new
      before = MiniRacerRs.counters(context)[:gc_count]
      context.eval("gc(); gc({ type: 'minor' }); gc()")
      p MiniRacerRs.counters(context)[:gc_count] - before
    RUBY

    assert_predicate status, :success?, err
    assert_equal "3\n", out
  end
end

class CountersReadTest < Minitest::Test
  include CounterHelpers

  def test_a_new_context_reports_every_key_at_zero
    counts = counters(MiniRacer::Context.new)

    assert_equal %i[dispatches callbacks bytes_to_js bytes_from_js js_ns gc_count gc_ns terminated], counts.keys
    assert_equal({ dispatches: 0, callbacks: 0, bytes_to_js: 0, bytes_from_js: 0, js_ns: 0, gc_count: 0, gc_ns: 0 },
                 counts.except(:terminated),)
    assert_equal NONE, counts[:terminated]
  end

  def test_reads_counts_while_another_thread_holds_the_context
    context = MiniRacer::Context.new
    inside = Queue.new
    release = Queue.new
    context.attach("hold", proc do
      inside << true
      release.pop
    end,)
    holder = Thread.new { context.eval("hold(); 1") }
    inside.pop
    during = Timeout.timeout(5) { counters(context) }
    release << true

    assert_equal 1, holder.value
    assert_equal({ dispatches: 2, callbacks: 1 }, during.slice(:dispatches, :callbacks))
  end

  def test_reads_counts_while_javascript_runs_on_another_thread
    context = MiniRacer::Context.new
    running = Thread.new { context.eval("while (true) {}") }
    running.report_on_exception = false
    dispatched(context, 1)
    during = Timeout.timeout(5) { counters(context) }
    context.stop

    assert_raises(MiniRacer::ScriptTerminatedError) { running.join }
    assert_equal({ dispatches: 1, bytes_from_js: 0, js_ns: 0 }, during.slice(:dispatches, :bytes_from_js, :js_ns))
    assert_operator counters(context)[:js_ns], :>, 0
  end

  def test_keeps_the_last_counts_after_dispose
    context = MiniRacer::Context.new
    context.eval("1 + 1")
    last = counters(context)
    context.dispose
    assert_raises(MiniRacer::ContextDisposedError) { context.eval("1") }

    assert_equal last, counters(context)
  end

  def test_refuses_anything_but_a_context
    [nil, 42, Object.new, MiniRacer::Snapshot.new("1")].each do |other|
      assert_raises(TypeError) { MiniRacerRs.counters(other) }
    end
    subclassed = Class.new(MiniRacer::Context).new
    subclassed.eval("1")

    assert_equal 1, counters(subclassed)[:dispatches]
  end
end
