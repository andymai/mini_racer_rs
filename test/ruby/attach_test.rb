# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class AttachTest < Minitest::Test
  include RubyProcess

  def setup
    @context = MiniRacer::Context.new
  end

  def teardown
    @context&.dispose
  end

  def test_a_callback_calls_back_into_its_context_two_levels_deep
    @context.eval("function twice(x) { return 2 * x }")
    @context.attach("inner", proc { |x| @context.call("twice", x) + 1 })
    @context.attach("outer", proc { |x| @context.eval("inner(#{x})") * 10 })

    assert_equal 70, @context.eval("outer(3)")
    assert_equal 7, @context.eval("inner(3)")
  end

  def test_an_exception_escaping_a_nested_eval_reaches_the_outer_caller
    @context.attach("inner", proc { raise KeyError, "deep" })
    @context.attach("outer", proc { @context.eval("inner()") })

    error = assert_raises(KeyError) { @context.eval("outer()") }
    assert_equal "deep", error.message
    assert_equal 2, @context.eval("1 + 1")
  end

  def test_another_thread_waits_for_the_callback_to_finish
    other = nil
    start = proc do
      other = Thread.new { @context.eval("'second'") }
      sleep 0.05
      other.alive?
    end
    @context.attach("start", start)

    assert @context.eval("start()")
    assert_equal "second", other.value
  end

  def test_a_failed_attach_still_uses_up_its_id
    assert_raises(TypeError) { @context.attach(:sym, proc { 1 }) }
    @context.attach("one", proc { 1 })

    assert_equal 1, @context.eval("one()")
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
