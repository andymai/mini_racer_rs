# frozen_string_literal: true

require "test_helper"

class VerboseExceptionsTest < Minitest::Test
  include RubyProcess

  # What V8 printed while a fresh process ran `script`, as mini_racer's
  # verbose TryCatch reports an exception that escapes a script.
  def reported(script)
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      def quietly
        yield
      rescue MiniRacer::Error, RuntimeError
        nil
      end
      #{script}
    RUBY
    assert_predicate status, :success?, err
    out
  end

  def test_an_escaping_exception_is_reported_where_v8_saw_it
    out = reported(<<~RUBY)
      c = MiniRacer::Context.new(verbose_exceptions: true)
      quietly { c.eval("throw new Error('boom')", filename: "x.js") }
      quietly { c.eval("let x = ;", filename: "p.js") }
      c.eval("function f() { throw new TypeError('bad') }")
      quietly { c.call("f") }
      c.attach("rb", proc { raise "ruby" })
      quietly { c.eval("rb()") }
    RUBY

    assert_equal <<~OUT, out
      x.js:0: Uncaught Error: boom
      p.js:8: Uncaught SyntaxError: Unexpected token ';'
      <eval>:15: Uncaught TypeError: bad
      <eval>:0: Uncaught Error: ruby
    OUT
  end

  def test_nothing_is_reported_when_caught_or_without_the_option
    out = reported(<<~RUBY)
      c = MiniRacer::Context.new(verbose_exceptions: true)
      c.eval("try { throw new Error('quiet') } catch (e) { 1 }")
      d = MiniRacer::Context.new(verbose_exceptions: false)
      quietly { d.eval("throw new Error('silent')") }
      e = MiniRacer::Context.new
      quietly { e.eval("throw new Error('silent')") }
    RUBY

    assert_equal "", out
  end

  def test_any_truthy_value_turns_it_on
    out = reported(<<~RUBY)
      c = MiniRacer::Context.new(verbose_exceptions: "yes")
      quietly { c.eval("throw new Error('str')") }
    RUBY

    assert_equal "<eval>:0: Uncaught Error: str\n", out
  end
end
