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

  # Deviation behavior:shared-source-heap-stats@ruby: mini_racer 0.22.1
  # reports `:external_memory` as 0 in all three contexts and the same
  # `:used_heap_size` in each, 14,578,448 bytes for this script, as each
  # keeps a copy of the source of its own. Here the second and third read
  # the one copy the process shares, outside V8's heap, which V8 counts in
  # `:external_memory` by its length in characters.
  def test_a_context_reading_a_shared_source_reports_it_as_external_memory
    out, err, status = run_ruby(STATS, env: { "MINI_RACER_RS_CODE_CACHE" => nil })

    assert_predicate status, :success?, err
    units, ((first_used, first_external), *later) = JSON.parse(out)

    assert_equal 0, first_external
    later.each do |used, external|
      assert_equal units, external
      assert_operator first_used - used, :>, units * 2 * 0.9
    end
  end

  # Deviation behavior:shared-source-heap-stats@ruby: with the switch set
  # before the first context, every context keeps its own copy and reports
  # it as mini_racer 0.22.1 does.
  def test_with_the_code_cache_off_every_context_counts_its_own_copy
    out, err, status = run_ruby(STATS, env: { "MINI_RACER_RS_CODE_CACHE" => "off" })

    assert_predicate status, :success?, err
    units, stats = JSON.parse(out)
    first_used = stats.first.first

    assert_equal [0, 0, 0], stats.map(&:last)
    stats.map(&:first).each { |used| assert_in_delta first_used, used, units * 0.1 }
  end
end
