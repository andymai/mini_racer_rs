# frozen_string_literal: true

require "test_helper"

class IdleGcTest < Minitest::Test
  include RubyProcess

  # The low memory collections V8 traces in a fresh process running `script`.
  def collections(script)
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      MiniRacer::Platform.set_flags!(:trace_gc)
      #{script}
    RUBY
    assert_predicate status, :success?, err
    out.lines.count { |line| line.include?("low memory notification") }
  end

  # As mini_racer's runner: once idle that long after a request, and not
  # again until the next one; a context nothing has asked of yet never does.
  def test_a_context_collects_once_after_a_request_and_never_before_one
    once = collections("c = MiniRacer::Context.new(ensure_gc_after_idle: 20); c.eval('1'); sleep 0.3")

    assert_operator once, :>, 0
    assert_equal once, collections("c = MiniRacer::Context.new(ensure_gc_after_idle: 20); c.eval('1'); sleep 0.8")
    assert_equal 0, collections("MiniRacer::Context.new(ensure_gc_after_idle: 20); sleep 0.3")
    assert_equal 0, collections("c = MiniRacer::Context.new; c.eval('1'); sleep 0.3")
  end
end
