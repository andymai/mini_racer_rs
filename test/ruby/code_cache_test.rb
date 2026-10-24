# frozen_string_literal: true

require "test_helper"
require "json"
require "mini_racer"

# MiniRacerRs.code_cache reads the process's code cache and shared sources
# from atomics: no dispatch, no context. mini_racer 0.22.2 has no counterpart.
class CodeCacheTest < Minitest::Test
  include RubyProcess

  KEYS = %i[entries bytes hits builds rebuilds refused shared_sources shared_bytes].freeze
  NONE = KEYS.to_h { |key| [key, 0] }.freeze

  def test_reads_the_process_counts_as_a_frozen_hash
    assert_respond_to MiniRacerRs, :code_cache
    stats = MiniRacerRs.code_cache

    assert_predicate stats, :frozen?
    assert_equal KEYS, stats.keys
    stats.each_value { |count| assert_operator count, :>=, 0 }
  end

  # A script of a megabyte or more, loaded by one context, then a second that
  # caches it, then a third that reads the cache; the second's next call
  # builds the cache again. The counts are the process's, so a fresh Ruby.
  LOADS = <<~'RUBY'
    require "json"
    require "mini_racer"
    source = "/*#{"x" * (1 << 20)}*/ 41 + 1"
    stats = [MiniRacerRs.code_cache]
    contexts = Array.new(3) { MiniRacer::Context.new }
    contexts.each do |context|
      context.eval(source)
      stats << MiniRacerRs.code_cache
    end
    contexts[1].eval("1")
    stats << MiniRacerRs.code_cache
    puts JSON.generate([source.bytesize, stats])
  RUBY

  def loads(env = {})
    out, err, status = run_ruby(LOADS, env: env)

    assert_predicate status, :success?, err
    JSON.parse(out, symbolize_names: true)
  end

  def test_counts_a_load_a_build_a_hit_and_a_rebuild
    bytesize, (fresh, loaded, built, hit, rebuilt) = loads

    assert_equal NONE, fresh
    assert_equal NONE.merge(shared_sources: 1, shared_bytes: bytesize), loaded
    assert_operator built[:bytes], :>, 0
    assert_equal loaded.merge(entries: 1, bytes: built[:bytes], builds: 1), built
    assert_equal built.merge(hits: 1), hit
    assert_operator rebuilt[:bytes], :>, 0
    assert_equal hit.merge(bytes: rebuilt[:bytes], rebuilds: 1), rebuilt
  end

  def test_counts_nothing_with_the_cache_off
    _, stats = loads("MINI_RACER_RS_CODE_CACHE" => "off")

    assert_equal [NONE] * 5, stats
  end
end
