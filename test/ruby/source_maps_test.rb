# frozen_string_literal: true

require "test_helper"
require "json"
require "mini_racer"

# behavior:source-maps@ruby. mini_racer 0.22.2 has no source maps: the frames
# of its MiniRacer::RuntimeError, which join the error's backtrace, give V8's
# positions in the evaluated source. With a map registered for a bundle, the
# frames of an error in that bundle give the original source's positions
# instead, keeping V8's function names; nothing JavaScript reads changes.
module SourceMapFixtures
  BASE64 = [*"A".."Z", *"a".."z", *"0".."9", "+", "/"].freeze
  SOURCES = ["./src/Header.jsx", "./src/App.jsx"].freeze
  ROOT = "webpack://app/"

  module_function

  def vlq(value)
    unsigned = value.negative? ? (-value << 1) | 1 : value << 1
    out = +""
    loop do
      digit = unsigned & 31
      unsigned >>= 5
      out << BASE64.fetch(unsigned.zero? ? digit : digit | 32)
      return out if unsigned.zero?
    end
  end

  # Every column of the bundle's line `l` maps to line 10 (l + 1), at the
  # column plus 100, of source `l % 2`.
  def map(bundle)
    previous = [0, 0, 0]
    lines = bundle.split("\n", -1).each_with_index.map do |text, line|
      generated = 0
      Array.new(text.encode("UTF-16LE").bytesize / 2) do |column|
        fields = [line % 2, 10 * (line + 1), column + 100]
        segment = vlq(column - generated) + fields.zip(previous).map { |value, last| vlq(value - last) }.join
        generated = column
        previous = fields
        segment
      end.join(",")
    end
    JSON.generate("version" => 3, "sourceRoot" => ROOT, "sources" => SOURCES, "sourcesContent" => ["x"],
                  "names" => [], "mappings" => lines.join(";"),)
  end

  def expected(line, column)
    "#{ROOT}#{SOURCES[line % 2]}:#{(10 * (line + 1)) + 1}:#{column + 101}"
  end

  def bundle(tag)
    <<~JS
      function Ue#{tag}() { throw new Error("boom #{tag}"); }
      function Header#{tag}(props) { return Ue#{tag}(); }
      function render#{tag}() { return [1].map(function mapper() { return Header#{tag}({}); })[0]; }
      async function deep#{tag}() { await null; return Header#{tag}({}); } async function outer#{tag}() { return await deep#{tag}(); }
      globalThis.render = render#{tag}; globalThis.outer = outer#{tag}; /*#{"pad " * 256}*/ /*0*/
    JS
  end

  # The same source with the bundle's marker comment changed, which no map
  # matches and which moves no position.
  def unmatched(source)
    source.sub("/*0*/", "/*1*/")
  end

  # A baseline backtrace frame relocated by `expected` when it falls in the
  # bundle, which starts at `start` and spans five lines.
  def mapped(frame, name, start)
    location = frame[/#{Regexp.escape(name)}:(\d+):(\d+)\)?\z/]
    return frame if location.nil? || frame.start_with?("JavaScript at async")

    line, column = Regexp.last_match.captures.map { _1.to_i - 1 }
    line -= start[0]
    return frame if line.negative? || line >= 5 || (line.zero? && column < start[1])

    column -= start[1] if line.zero?
    frame.sub(/#{Regexp.escape(name)}:\d+:\d+(\)?)\z/) { "#{expected(line, column)}#{Regexp.last_match(1)}" }
  end
end

class SourceMapsTest < Minitest::Test
  include SourceMapFixtures
  include RubyProcess

  PREFIX = "var global = global || this;\nvar self = self || this;\n"

  def setup
    @bundle = bundle("A")
    MiniRacerRs.register_source_map(@bundle, map(@bundle))
    @source = "#{PREFIX}#{@bundle}function outside() { return render(); }\n"
    @matched = MiniRacer::Context.new
    @matched.eval(@source)
    @baseline = MiniRacer::Context.new
    @baseline.eval(unmatched(@source))
  end

  def teardown
    @matched.dispose
    @baseline.dispose
  end

  def raised(&)
    yield

    flunk "no error"
  rescue MiniRacer::RuntimeError => e
    [e.message, e.backtrace.grep(/\AJavaScript /)]
  end

  def test_maps_the_frames_of_a_runtime_error_in_the_bundle
    ["render()", "outside()"].each do |call|
      message, frames = raised { @matched.eval(call) }
      baseline_message, baseline = raised { @baseline.eval(call) }

      assert_equal baseline_message, message
      # The last frame is the calling eval's own script.
      want = baseline[..-2].map { mapped(_1, "<eval>", [2, 0]) } + baseline[-1..]

      assert_equal want, frames, call
      assert_includes frames.first, "JavaScript at UeA (webpack://app/./src/Header.jsx:11:"
      assert_includes frames, "JavaScript at Array.map (<anonymous>)"
    end
  end

  def test_maps_a_call_and_an_awaited_rejection
    _, frames = raised { @matched.call("render") }
    _, baseline = raised { @baseline.call("render") }

    assert_equal(baseline.map { mapped(_1, "<eval>", [2, 0]) }, frames)
    _, frames = raised { @matched.eval_await("outer()") }
    _, baseline = raised { @baseline.eval_await("outer()") }

    assert_equal(baseline.map { mapped(_1, "<eval>", [2, 0]) }, frames)
    refute_empty frames.grep(/\AJavaScript at async outerA \(<eval>:/)
  end

  def test_keeps_what_javascript_reads
    _, frames = raised { @matched.eval("render()") }

    refute_empty frames.grep(/webpack:/)
    [
      "(() => { try { render() } catch (e) { return e.stack } })()",
      "(() => { const o = {}; Error.captureStackTrace(o); return o.stack })()",
      "(() => { Error.prepareStackTrace = (e, s) => s.map(String); " \
      "try { render() } catch (e) { return e.stack } finally { delete Error.prepareStackTrace } })()",
    ].each do |probe|
      assert_equal @baseline.eval(probe), @matched.eval(probe), probe
    end
  end

  def test_leaves_other_errors_and_contexts_alone
    plain = "throw new Error('plain')"

    assert_equal(raised { @baseline.eval(plain) }, raised { @matched.eval(plain) })
    error = assert_raises(MiniRacer::ParseError) { @matched.eval("1 +") }
    assert_equal "Uncaught SyntaxError: Unexpected end of input at <eval>:1:3", error.message
    # A context that never evaluated the bundle has nothing to map.
    other = MiniRacer::Context.new
    other.eval(unmatched(@bundle))
    _, frames = raised { other.eval("render()") }

    assert_empty frames.grep(/webpack:/)
  end
end

# What MiniRacerRs.register_source_map refuses, and how many bundles it keeps.
class SourceMapRegistrationTest < Minitest::Test
  include SourceMapFixtures
  include RubyProcess

  # Past the registry's 1 KiB minimum, on one line.
  def padded(text)
    "#{text}#{" " * 1024}"
  end

  def test_refuses_a_map_that_does_not_parse_or_fit
    {
      "{" => /not valid JSON/,
      %({"version":2,"sources":[],"mappings":""}) => /version is not 3/,
      %({"version":3,"sections":[]}) => /index source maps/,
      %({"version":3,"sources":["a.js"],"mappings":";;;AAAA"}) => /maps line 4, but the bundle has 2 lines/,
    }.each do |map, message|
      error = assert_raises(ArgumentError) { MiniRacerRs.register_source_map(padded("var x = 1;\n"), map) }
      assert_match message, error.message
    end
    error = assert_raises(ArgumentError) { MiniRacerRs.register_source_map("\xFF".b * 1024, map("x")) }
    assert_equal "the bundle is not valid UTF-8", error.message
    assert_raises(TypeError) { MiniRacerRs.register_source_map(1, map("x")) }
    assert_raises(TypeError) { MiniRacerRs.register_source_map("x", nil) }
  end

  # A map past 256 MiB is refused by its length, before it is copied or read.
  def test_refuses_a_map_past_256_mib
    map = "{#{" " * (256 << 20)}}"
    error = assert_raises(ArgumentError) { MiniRacerRs.register_source_map(padded("var x = 1;\n"), map) }
    assert_equal "there are more than 268435456 bytes in the source map", error.message
  end

  # A bundle short enough to turn up in unrelated code is refused.
  def test_refuses_a_bundle_shorter_than_1_kib
    ["", "throw new Error(", "x" * 1023].each do |bundle|
      error = assert_raises(ArgumentError) { MiniRacerRs.register_source_map(bundle, map(bundle)) }
      assert_equal "the bundle is shorter than 1024 bytes", error.message
    end
  end

  # Context#eval sends an ISO-8859-1 String as Latin-1 and a UTF-16LE one as
  # UTF-16, which are never searched for a bundle.
  def test_refuses_a_bundle_in_an_encoding_eval_does_not_send_as_utf8
    bundle = padded("var x = 1;\n")
    %w[ISO-8859-1 UTF-16LE].each do |encoding|
      error = assert_raises(ArgumentError) do
        MiniRacerRs.register_source_map(bundle.encode(encoding), map(bundle))
      end
      assert_equal "the bundle must be UTF-8, not #{encoding}", error.message
    end
  end

  # A source or the root is printed into a backtrace line, where a control
  # character could add a line or disguise one.
  def test_refuses_control_characters_in_sources
    bundle = padded("var x = 1;\n")
    ["x\r\e[2K/app/models/user.rb:42:in `save'", "a\nb", "a\u2028b", "a\u202Eb"].each do |text|
      [
        JSON.generate("version" => 3, "sources" => [text], "mappings" => "AAAA"),
        JSON.generate("version" => 3, "sourceRoot" => text, "sources" => ["a.js"], "mappings" => "AAAA"),
      ].each do |map|
        error = assert_raises(ArgumentError) { MiniRacerRs.register_source_map(bundle, map) }
        assert_match(/control character/, error.message)
      end
    end
  end

  def test_maps_at_most_eight_bundles
    _, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      map = '{"version":3,"sources":["a.js"],"mappings":"AAAA"}'
      bundle = ->(i) { "var b\#{i} = 1;\#{" " * 1024}" }
      8.times { |i| MiniRacerRs.register_source_map(bundle.(i), map) }
      MiniRacerRs.register_source_map(bundle.(0), map)
      begin
        MiniRacerRs.register_source_map(bundle.(8), map)
        abort "a ninth bundle was taken"
      rescue ArgumentError => e
        abort e.message unless e.message == "at most 8 bundles can have a source map"
      end
    RUBY
    assert_predicate status, :success?, err
  end
end
