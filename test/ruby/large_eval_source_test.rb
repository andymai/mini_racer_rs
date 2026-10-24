# frozen_string_literal: true

require "test_helper"
require "json"
require "mini_racer"

class LargeEvalSourceTest < Minitest::Test
  include RubyProcess

  # A second thread counts its turns while the first is inside eval and the
  # eval has not reached the runner yet, which for a 32 MiB source is the
  # copy of its bytes. Its first such turn changes the source's last byte,
  # then collects and compacts.
  COPY = <<~'RUBY'
    require "json"
    require "mini_racer"
    context = MiniRacer::Context.new
    source = "/*#{"a" * (32 << 20)}*/ 1"
    evaluating = Thread.current
    sent = MiniRacerRs.counters(context)[:dispatches]
    done = false
    ticks = 0
    ticker = Thread.new do
      until done
        if evaluating.backtrace_locations(0, 1).first&.base_label == "eval" &&
           MiniRacerRs.counters(context)[:dispatches] == sent
          if ticks.zero?
            source.setbyte(source.bytesize - 1, "2".ord)
            GC.start
            GC.compact
          end
          ticks += 1
        end
        Thread.pass
      end
    end
    result = context.eval(source)
    done = true
    ticker.join
    puts JSON.generate([ticks, result, source[-1]])
  RUBY

  def test_other_threads_run_while_a_large_source_is_copied
    out, err, status = run_ruby(COPY)

    assert_predicate status, :success?, err
    ticks, result, last = JSON.parse(out)

    assert_operator ticks, :>, 0, "no other thread ran while the source was copied"
    assert_equal "2", last
    assert_equal 1, result, "the eval read the source as changed during its copy"
  end

  TEXT = "aé€\u{1F600}"

  def repeated(text)
    text * (((1 << 20) / text.bytesize) + 1)
  end

  def literal(body, encoding)
    quote = "'".encode(encoding)
    quote + body + quote
  end

  # mini_racer sends ISO-8859-1 and UTF-16LE in their own encoding and the
  # bytes of anything else as UTF-8, which V8 decodes with replacements.
  def test_large_sources_read_as_their_encoding_says
    context = MiniRacer::Context.new
    binary = repeated("#{TEXT}\xFF".b)
    latin1 = repeated("aéÿ".encode("ISO-8859-1"))
    {
      repeated(TEXT) => repeated(TEXT),
      binary => binary.dup.force_encoding("UTF-8").scrub,
      latin1 => latin1.encode("UTF-8"),
      repeated(TEXT).encode("UTF-16LE") => repeated(TEXT),
    }.each do |body, expected|
      source = literal(body, body.encoding)

      assert_operator source.bytesize, :>=, 1 << 20
      assert_equal expected, context.eval(source), body.encoding.name
    end
  end

  def test_a_large_odd_length_utf16_source_is_refused_as_a_small_one
    context = MiniRacer::Context.new
    refused = lambda do |body|
      error = assert_raises(MiniRacer::Error) do
        context.eval("#{literal(body, "UTF-16LE").b}x".force_encoding("UTF-16LE"))
      end
      [error.class, error.message]
    end

    assert_equal refused.call("a".encode("UTF-16LE")), refused.call(repeated(TEXT).encode("UTF-16LE"))
  end
end
