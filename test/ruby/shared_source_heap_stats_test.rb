# frozen_string_literal: true

require "test_helper"
require "json"

class SharedSourceHeapStatsTest < Minitest::Test
  include RubyProcess

  # Three contexts load one script of about 1.6 million UTF-16 units, two
  # bytes a unit in V8 for its astral character, and each reports its heap
  # statistics after a collection.
  STATS = <<~'RUBY'
    require "json"
    require "mini_racer"
    functions = Array.new(40_000) { |i| "function f#{i}(a) { return a + #{i}; }" }
    source = "// \u{1F600}\n#{functions.join("\n")}\nf0(1);"
    stats = Array.new(3) do
      context = MiniRacer::Context.new
      context.eval(source)
      context.low_memory_notification
      context.heap_stats.slice(:used_heap_size, :external_memory).values
    end
    puts JSON.generate([source.encode("UTF-16LE").bytesize / 2, stats])
  RUBY

  def reported(code_cache)
    out, err, status = run_ruby(STATS, env: { "MINI_RACER_RS_CODE_CACHE" => code_cache })

    assert_predicate status, :success?, err
    JSON.parse(out)
  end

  # Deviation behavior:shared-source-heap-stats@ruby: mini_racer 0.22.2
  # reports `:external_memory` as 0 in all three contexts and the same
  # `:used_heap_size` in each, 14,578,448 bytes for this script, as each
  # keeps a copy of the source of its own. Here all three, the first
  # included, read the one copy the process shares, outside V8's heap, which
  # V8 counts in `:external_memory` by its length in characters.
  def test_every_context_reading_a_shared_source_reports_it_as_external_memory
    units, shared = reported(nil)
    _, own = reported("off")

    assert_equal [units] * 3, shared.map(&:last)
    shared.zip(own).each do |(used, _), (own_used, _)|
      assert_operator own_used - used, :>, units * 2 * 0.9
    end
  end

  # A script of 128 KiB to a megabyte is cached from its second load, and
  # every context still keeps its own copy of the source, reporting
  # `:external_memory` as 0 as mini_racer 0.22.2 does.
  def test_a_script_under_a_megabyte_is_never_shared
    out, err, status = run_ruby(<<~'RUBY')
      require "json"
      require "mini_racer"
      functions = Array.new(6_000) { |i| "function f#{i}(a) { return a + #{i}; }" }
      source = "// \u{1F600}\n#{functions.join("\n")}\nf0(1);"
      abort "#{source.bytesize} bytes" unless source.bytesize.between?(128 << 10, 1 << 20)
      stats = Array.new(3) do
        context = MiniRacer::Context.new
        context.eval(source)
        context.heap_stats[:external_memory]
      end
      puts JSON.generate(stats)
    RUBY

    assert_predicate status, :success?, err
    assert_equal [0, 0, 0], JSON.parse(out)
  end

  # Deviation behavior:shared-source-heap-stats@ruby: with the switch set
  # before the first context, every context keeps its own copy and reports
  # it as mini_racer 0.22.2 does.
  def test_with_the_code_cache_off_every_context_counts_its_own_copy
    units, stats = reported("off")
    first_used = stats.first.first

    assert_equal [0, 0, 0], stats.map(&:last)
    stats.map(&:first).each { |used| assert_in_delta first_used, used, units * 0.1 }
  end
end
