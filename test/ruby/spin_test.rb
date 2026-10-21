# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class SpinTest < Minitest::Test
  NAP = "globalThis.nap = (ms) => { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms); return ms }"

  def thread_cpu = Process.clock_gettime(Process::CLOCK_THREAD_CPUTIME_ID)

  # Spinning through the default budget for every one of these answers would
  # take about 70 us of this thread's CPU per call.
  def test_a_context_whose_answers_outlast_the_spin_budget_stops_spinning
    context = MiniRacer::Context.new
    context.eval(NAP)
    5.times { context.call("nap", 2) }
    started = thread_cpu
    20.times { context.call("nap", 2) }

    assert_operator (thread_cpu - started) / 20 * 1e6, :<, 40
  ensure
    context&.dispose
  end
end
