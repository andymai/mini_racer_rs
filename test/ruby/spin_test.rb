# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class SpinTest < Minitest::Test
  include RubyProcess

  # A 1 ms budget without the GVL makes a spinning wait cost the calling
  # thread about 1,000 us of CPU per call, far above what parking costs on
  # any machine. The budget is read once per process, hence the child.
  def test_a_context_whose_answers_outlast_the_spin_budget_stops_spinning
    out, err, status = run_ruby(<<~RUBY, env: { "MINI_RACER_RS_SPIN_US" => "0,1000" })
      require "mini_racer"
      context = MiniRacer::Context.new
      context.eval("globalThis.nap = (ms) => { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms); return ms }")
      5.times { context.call("nap", 3) }
      started = Process.clock_gettime(Process::CLOCK_THREAD_CPUTIME_ID)
      20.times { context.call("nap", 3) }
      puts ((Process.clock_gettime(Process::CLOCK_THREAD_CPUTIME_ID) - started) / 20 * 1e6).round
    RUBY

    assert_predicate status, :success?, err
    assert_operator Integer(out), :<, 300
  end
end
