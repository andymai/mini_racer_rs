# frozen_string_literal: true

require "test_helper"
require "mini_racer"

# Where callbacks behave differently from mini_racer 0.22.1 by design; each
# test names the compat.toml deviation it shows.
class CallbackDeviationTest < Minitest::Test
  include RubyProcess

  def setup
    @context = MiniRacer::Context.new
  end

  def teardown
    @context&.dispose
  end

  # Deviation behavior:object-ids@ruby: mini_racer 0.22.1 sends each occurrence
  # of a Time or MiniRacer::Binary as a separate object.
  def test_a_repeated_time_or_binary_reaches_javascript_as_one_object
    @context.eval("globalThis.same = (a, b) => a === b")
    time = Time.at(0)
    binary = MiniRacer::Binary.new("ab")

    assert @context.call("same", time, time)
    assert @context.call("same", binary, binary)
    refute @context.call("same", time, Time.at(0))
  end

  # Deviation behavior:callback-reentry@ruby: mini_racer 0.22.1 lets any fiber
  # on the thread call in, and hangs when a signal trap calls in while a
  # callback waits on the context.
  def test_only_the_callbacks_own_fiber_calls_back_in
    @context.attach("other", proc { Enumerator.new { |y| y << @context.eval("1") }.next })

    assert_raises(ThreadError) { @context.eval("other()") }
    out, err, status = run_ruby(<<~RUBY)
      Thread.new { sleep 10; warn "timed out"; exit!(99) }
      require "mini_racer"
      context = MiniRacer::Context.new
      trapped = nil
      waiting = false
      Signal.trap("USR1") { trapped ||= (context.eval("1") rescue $!.class) }
      context.attach("waits", proc {
        waiting = true
        context.eval("const t = Date.now(); while (Date.now() - t < 1000) {}; 7")
      })
      # A trap that ran in the callback's own Ruby code would be a legitimate
      # reentry, so signals start once the callback is waiting, and repeat in
      # case one is delivered late.
      Thread.new do
        sleep 0.01 until waiting
        sleep 0.05
        until trapped
          Process.kill("USR1", Process.pid)
          sleep 0.05
        end
      end
      p [context.eval("waits()"), trapped, context.eval("2 + 2")]
    RUBY

    assert_predicate status, :success?, err
    assert_equal "[7, ThreadError, 4]\n", out
  end

  # Deviation behavior:callback-error-text@ruby: mini_racer 0.22.1 reads the
  # message of an exception a callback raises as Latin-1, so "café" reaches
  # JavaScript as "cafÃ©".
  def test_a_callback_error_message_keeps_its_encoding
    @context.attach("raises", proc { raise "café 東" })

    assert_equal "café 東", @context.eval("try { raises() } catch (e) { e.message }")
  end

  # Deviation behavior:callback-jump@ruby: mini_racer 0.22.1 deadlocks when a
  # callback leaves by throw or Thread#kill. Here the script is terminated and
  # the jump goes on once the eval has returned.
  def test_a_jump_out_of_a_callback_ends_the_script_and_continues
    out, err, status = run_ruby(<<~RUBY)
      Thread.new { sleep 10; warn "timed out"; exit!(99) }
      require "mini_racer"
      context = MiniRacer::Context.new
      context.attach("thrower", proc { throw :out, 5 })
      context.attach("killer", proc { Thread.current.kill })
      p catch(:out) { context.eval("thrower(); globalThis.ran = true; 'not reached'") }
      p context.eval("typeof ran")
      p Thread.new { context.eval("killer(); 'not reached'") }.join.value
      p context.eval("1 + 2")
    RUBY

    assert_predicate status, :success?, err
    assert_equal "5\n\"undefined\"\nnil\n3\n", out
  end
end
