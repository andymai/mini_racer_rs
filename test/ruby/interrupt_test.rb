# frozen_string_literal: true

require "test_helper"
require "mini_racer"
require "timeout"

class InterruptTest < Minitest::Test
  include RubyProcess

  def elapsed
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end

  def test_timeout_stops_a_running_script
    context = MiniRacer::Context.new
    seconds = elapsed do
      assert_raises(Timeout::Error) { Timeout.timeout(0.2) { context.eval("while (true) {}") } }
    end

    assert_operator seconds, :<, 5
    assert_equal 2, context.eval("1 + 1")
  end

  def test_thread_kill_stops_a_waiting_thread
    context = MiniRacer::Context.new
    thread = Thread.new { context.eval("while (true) {}") }
    sleep 0.1
    thread.kill

    assert thread.join(5), "the killed thread is still waiting"
    assert_equal 3, context.eval("1 + 2")
  end

  def test_signal_trap_does_not_terminate_javascript
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      hits = 0
      Signal.trap("USR1") { hits += 1 }
      context = MiniRacer::Context.new
      Thread.new { sleep 0.05; Process.kill("USR1", Process.pid) }
      p context.eval("const end = Date.now() + 300; let n = 0; while (Date.now() < end) n++; n > 0")
      p hits
    RUBY

    assert_predicate status, :success?, err
    assert_equal "true\n1\n", out
  end

  def test_dispose_from_a_trap_during_eval_is_refused
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      context = MiniRacer::Context.new
      Signal.trap("USR1") do
        context.dispose
      rescue MiniRacer::RuntimeError => e
        puts e.message
      end
      Thread.new { sleep 0.05; Process.kill("USR1", Process.pid) }
      p context.eval("const end = Date.now() + 300; while (Date.now() < end); 42")
    RUBY

    assert_predicate status, :success?, err
    assert_equal "context dispose: Device or resource busy\n42\n", out
  end

  def test_forked_child_reports_an_inherited_context
    skip "fork is unavailable" unless Process.respond_to?(:fork)

    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      context = MiniRacer::Context.new
      context.eval("1")
      pid = fork do
        begin
          puts "evaluated " + context.eval("1").to_s
        rescue MiniRacer::RuntimeError => e
          puts e.message
        end
        $stdout.flush
        exit!(0)
      end
      Process.wait(pid)
      p context.eval("2")
    RUBY

    assert_predicate status, :success?, err
    assert_match(/\Asingle-threaded runner: \S.*\n2\n\z/, out)
  end
end
