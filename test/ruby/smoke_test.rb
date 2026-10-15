# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class SmokeTest < Minitest::Test
  include RubyProcess

  def setup
    @context = MiniRacer::Context.new
  end

  def test_eval_returns_primitives
    assert_equal 2, @context.eval("1 + 1")
    assert_in_delta 1.5, @context.eval("1.5")
    assert_equal 9_007_199_254_740_993, @context.eval("2 ** 53") + 1
    assert_equal 2**70, @context.eval("2 ** 70")
    assert_nil @context.eval("null")
    assert_nil @context.eval("undefined")
    assert @context.eval("1 < 2")
    assert_equal "héllo 東京 😀", @context.eval("'h\\u00e9llo ' + '東京' + ' 😀'")
    assert_equal Encoding::UTF_8, @context.eval("'ascii'").encoding
  end

  def test_intl_uses_bundled_icu
    assert_equal "28/4/2021", @context.eval("new Date('April 28 2021').toLocaleDateString('es-MX')")
  end

  def test_errors_map_to_upstream_classes
    assert_raises(MiniRacer::ParseError) { @context.eval("1 +") }
    error = assert_raises(MiniRacer::RuntimeError) { @context.eval("throw new Error('boom')") }

    assert_equal "Error: boom", error.message
    assert_equal 3, @context.eval("1 + 2")
  end

  def test_context_options_are_rejected_until_supported
    assert_raises(NotImplementedError) { MiniRacer::Context.new(timeout: 10) }
  end

  def test_flag_arguments_are_validated
    assert_raises(ArgumentError) { MiniRacer::Platform.set_flags!("bad\0flag") }
    assert_raises(ArgumentError) { MiniRacer::Platform.set_flags!("a" * 254) }
  end

  def test_flags_apply_before_initialization
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      MiniRacer::Platform.set_flags! :use_strict
      MiniRacer::Platform.set_flags! :stress_snapshot
      context = MiniRacer::Context.new
      puts context.eval("1 + 1")
      begin
        context.eval("x = 1")
      rescue MiniRacer::RuntimeError => e
        puts e.class
      end
      begin
        MiniRacer::Platform.set_flags! :expose_gc
      rescue MiniRacer::PlatformAlreadyInitialized => e
        puts e.message
      end
    RUBY

    assert_predicate status, :success?, err
    assert_equal "2\nMiniRacer::RuntimeError\nplatform already initialized\n", out
  end
end
