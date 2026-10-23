# frozen_string_literal: true

require "test_helper"
require "mini_racer"

# How deep mini_racer nests values each way: Ruby values it sends, a call's
# arguments and a callback's result, below 512 levels; a callback's arguments
# it decodes below 96.
class NestingTest < Minitest::Test
  include RubyProcess

  REFUSED = "Context.call: max depth exceeded"

  def setup
    @context = MiniRacer::Context.new
    @context.eval(<<~JS)
      globalThis.id = (...a) => a
      globalThis.depth = (x) => {
        let d = 0
        while (x && typeof x == 'object') { x = Array.isArray(x) ? x[0] : x.k; d++ }
        return d
      }
      globalThis.nested = (n) => { let v = []; for (let i = 1; i < n; i++) v = [v]; return v }
    JS
  end

  def teardown
    @context&.dispose
  end

  def arrays(levels, inner = []) = Array.new(levels).reduce(inner) { |value, _| [value] }

  def hashes(levels, inner = {}) = Array.new(levels).reduce(inner) { |value, _| { "k" => value } }

  def refused(*args) = assert_raises(MiniRacer::RuntimeError) { @context.call("id", *args) }.message

  # A call's arguments are one level down, so an argument can hold 511 levels
  # below itself.
  def test_an_argument_nested_past_512_levels_is_refused
    assert_equal 512, @context.call("depth", arrays(511))
    assert_equal 512, @context.call("depth", hashes(511))
    assert_equal 511, @context.call("depth", arrays(511, 1))
    [arrays(512), hashes(512), arrays(512, 1), hashes(512, 1), [1, arrays(511)], arrays(20_000)].each do |deep|
      assert_equal REFUSED, refused(deep)
    end
    assert_equal Encoding::BINARY, refused(arrays(512)).encoding
    assert_equal 2, @context.eval("1 + 1")
  end

  # mini_racer walks the values in order, so what comes first is refused
  # first, and it checks a hash's keys before it writes one.
  def test_a_value_before_one_too_deep_is_read_and_one_after_is_not
    stamps = 0
    stamp = Object.new
    stamp.define_singleton_method(:to_time) { (stamps += 1) && Time.at(0) }

    assert_equal "Context.call: unsupported type Object", refused(Object.new, arrays(512))
    assert_equal REFUSED, refused(stamp, arrays(512), stamp, Object.new)
    assert_equal 1, stamps
    assert_equal "Context.call: TODO serialize as Map", refused(arrays(511, { [1] => 2 }))
  end

  def test_a_callback_result_nested_past_512_levels_raises_an_internal_error
    @context.attach("made", proc { |levels| arrays(levels) })

    assert_equal 513, @context.eval("depth(made(512))")
    error = assert_raises(MiniRacer::InternalError) { @context.eval("made(513)") }
    assert_equal "max depth exceeded", error.message
    assert_equal ["max depth exceeded", "Error"],
                 @context.eval("try { made(20000) } catch (e) { [e.message, e.constructor.name] }")
  end

  def test_an_internal_error_that_cannot_be_made_raises_what_its_initialize_raised
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      class MiniRacer::InternalError
        def initialize(*) = raise("init boom")
      end
      context = MiniRacer::Context.new
      deep = Array.new(600).reduce([]) { |value, _| [value] }
      context.attach("deep", proc { deep })
      begin
        context.eval("deep()")
      rescue => e
        puts "\#{e.class}: \#{e.message}"
      end
      puts context.eval("try { deep() } catch (e) { e.message }")
    RUBY

    assert_predicate status, :success?, err
    assert_equal "RuntimeError: init boom\ninit boom\n", out
  end

  # mini_racer decodes a callback's arguments array at its limit, where it
  # decodes a result one level inside its reply, so either can nest 96 levels.
  def test_a_callback_argument_and_a_result_each_nest_96_levels
    @context.attach("levels", proc { |*args| args.map { |arg| arg.flatten.empty? } })

    assert_equal [true, true], @context.eval("levels(nested(96), nested(96))")
    error = assert_raises(MiniRacer::RuntimeError) { @context.eval("levels(1, nested(97))") }
    assert_equal "too much recursion", error.message
    assert_equal arrays(95), @context.eval("nested(96)")
    assert_raises(MiniRacer::RuntimeError) { @context.eval("nested(97)") }
  end
end
