# frozen_string_literal: true

require "test_helper"
require "json"

class SharedSourcesTest < Minitest::Test
  include RubyProcess

  # About 1.6 MB of UTF-8, which one astral character makes V8 keep two bytes
  # a unit. Each context reports its result and its heap after a collection.
  HEAPS = <<~'RUBY'
    require "json"
    require "mini_racer"
    functions = Array.new(40_000) { |i| "function f#{i}(a) { return a + #{i}; }" }
    source = "// \u{1F600}\n#{functions.join("\n")}\nf0(1);"
    contexts = Array.new(3) { MiniRacer::Context.new }
    results = contexts.map { |context| context.eval(source) }
    heaps = contexts.map do |context|
      context.low_memory_notification
      context.heap_stats[:used_heap_size]
    end
    puts JSON.generate([source.encode("UTF-16LE").bytesize, results, heaps])
  RUBY

  def outcome(script, code_cache)
    out, err, status = run_ruby(script, env: { "MINI_RACER_RS_CODE_CACHE" => code_cache })

    assert_predicate status, :success?, err
    JSON.parse(out)
  end

  # Every context loading a large script, the first included, reads its
  # source from the copy they all share; with the code cache off, each keeps
  # one on its own heap.
  def test_a_large_script_keeps_no_copy_on_any_heap
    bytes, results, heaps = outcome(HEAPS, nil)
    _, _, own = outcome(HEAPS, "off")

    assert_equal [1, 1, 1], results
    heaps.zip(own).each do |heap, own_heap|
      assert_operator own_heap - heap, :>, bytes * 0.9,
                      "a context's heap is #{own_heap - heap} bytes smaller than with a copy of its own"
    end
  end

  # A UTF-16LE source with lone surrogates in a comment and a string, which
  # mini_racer sends to V8 unit for unit, and probes of what V8 read.
  LONE = <<~'RUBY'
    require "json"
    require "mini_racer"
    def utf16(text) = text.encode("UTF-16LE")
    lone = [0xD800].pack("v").force_encoding("UTF-16LE")
    source = utf16("function fill(n) { return n + 1; }\n" * 20_000) +
             utf16("function withLone() { /* ") + lone + utf16(" */ return 'a") + lone + utf16("b'; }\n") +
             utf16("function thrower() { throw new Error('lone'); }\nwithLone().length")
    probes = [
      "[withLone().length, withLone().charCodeAt(1), withLone.toString().length, " \
      "withLone.toString().charCodeAt(withLone.toString().indexOf('/* ') + 3)]",
      "(() => { try { thrower() } catch (e) { return e.stack } })()",
      "(() => { Error.prepareStackTrace = (_, sites) => sites.slice(0, 1).map((site) => [site.getLineNumber(), " \
      "site.getColumnNumber(), site.getPosition(), site.getScriptHash()]); " \
      "try { thrower() } catch (e) { return e.stack } finally { delete Error.prepareStackTrace } })()",
    ]
    contexts = Array.new(3) { MiniRacer::Context.new }
    outcomes = contexts.map { |context| [context.eval(source), *probes.map { |probe| context.eval(probe) }] }
    heaps = contexts.map do |context|
      context.low_memory_notification
      context.heap_stats[:used_heap_size]
    end
    puts JSON.generate([source.bytesize, outcomes, heaps])
  RUBY

  # What mini_racer 0.22.2 returns for LONE's source and probes, in each of
  # its three contexts.
  LONE_OUTCOME = [
    3,
    [3, 55_296, 45, 55_296],
    "Error: lone\n    at thrower (<eval>:20002:28)\n    at <eval>:1:16\n    at <eval>:1:59",
    [[20_002, 28, 700_073, "0bf1bbf731eccea5f6e88bd0b42a4d6663b26feabc0365f0714fb6f85b554a52"]],
  ].freeze

  def test_a_shared_utf16_source_reads_its_lone_surrogates_as_mini_racer_does
    bytes, outcomes, heaps = outcome(LONE, nil)
    _, _, own = outcome(LONE, "off")

    assert_equal [LONE_OUTCOME] * 3, outcomes
    heaps.zip(own).each do |heap, own_heap|
      assert_operator own_heap - heap, :>, bytes * 0.9, "a context read a copy of its own"
    end
  end

  # Two UTF-8 sources of over a megabyte, one past ASCII only in Latin-1 and
  # one with characters of every width up to astral, each starting at a
  # shifting offset, so the runs a source is converted in end inside them.
  # Two contexts load each and report the function's length in units, a
  # digest of its text and their external memory.
  WIDTHS = <<~'RUBY'
    require "json"
    require "mini_racer"
    def comment(characters, count) = Array.new(count) { |i| ("a" * (i % 5)) + characters[i % characters.size] }.join
    latin1 = "function latin1() { /* #{comment(%W[\u00E9 \u00FF \u0080], 300_000)} */ }\n1"
    wide = "function wide() { /* #{comment(%W[\u00FF \u0100 \u07FF \u0800 \uFFFD \u{10000} \u{1F600} \u{10FFFF}],
                                        250_000)} */ }\n1"
    digest = "(() => { const text = %s.toString(); let hash = 0; for (let i = 0; i < text.length; i++) " \
             "hash = (Math.imul(hash, 31) + text.charCodeAt(i)) | 0; return [text.length, hash]; })()"
    outcomes = [[latin1, "latin1"], [wide, "wide"]].map do |source, name|
      Array.new(2) do
        context = MiniRacer::Context.new
        [context.eval(source), context.eval(format(digest, name)), context.heap_stats[:external_memory]]
      end
    end
    puts JSON.generate([[latin1, wide].map { |source| [source.bytesize, source.encode("UTF-16LE").bytesize / 2] },
                        outcomes])
  RUBY

  # With the code cache off each context keeps a copy of its own, which V8
  # decodes from the UTF-8 itself; the shared copy reads the same.
  def test_a_shared_utf8_source_reads_as_v8_decodes_it
    sizes, shared = outcome(WIDTHS, nil)
    _, own = outcome(WIDTHS, "off")
    bytes, units = sizes.transpose
    read = ->(outcomes) { outcomes.map { |contexts| contexts.map { |context| context.take(2) } } }
    external = ->(outcomes) { outcomes.map { |contexts| contexts.map(&:last) } }

    assert_operator bytes.min, :>, 1 << 20
    assert_equal read.call(own), read.call(shared)
    assert_equal(units.map { |count| [count, count] }, external.call(shared))
    assert_equal [[0, 0], [0, 0]], external.call(own)
  end
end
