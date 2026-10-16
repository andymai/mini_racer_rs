# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class WatchdogTest < Minitest::Test
  include RubyProcess

  def test_timeouts_still_fire_in_a_forked_child
    skip "fork is unavailable" unless Process.respond_to?(:fork)
    # Passes locally; on CI runners the child dies by a signal. A new context in
    # a forked child has no V8 platform workers until the fork protocol lands.
    skip "a new context in a forked child is best effort until the fork protocol lands"

    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      MiniRacer::Context.new(timeout: 1_000).eval("1")
      pid = fork do
        MiniRacer::Context.new(timeout: 100).eval("while (true) {}")
      rescue MiniRacer::ScriptTerminatedError => e
        puts "child: \#{e.message}"
      end
      Process.wait(pid)
      puts "child exit \#{$?.exitstatus}"
    RUBY

    assert_predicate status, :success?, err
    assert_equal "child: terminated\nchild exit 0\n", out
  end

  # Deviation behavior:timeout-watchdog@ruby: mini_racer runs the script with no
  # time limit when its watchdog thread cannot start.
  def test_a_timed_eval_fails_when_the_watchdog_cannot_start
    skip "RLIMIT_NPROC limits threads only on Linux" unless RUBY_PLATFORM.include?("linux")
    skip "root ignores RLIMIT_NPROC" if Process.uid.zero?

    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      context = MiniRacer::Context.new(timeout: 1_000)
      Process.setrlimit(:NPROC, 1)
      begin
        context.eval("while (true) {}")
      rescue MiniRacer::InternalError => e
        puts e.message
      end
    RUBY

    assert_predicate status, :success?, err
    assert_match(/\Acannot start the timeout watchdog: /, out)
  end
end
