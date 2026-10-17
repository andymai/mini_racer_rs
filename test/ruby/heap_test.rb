# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class HeapTest < Minitest::Test
  STATS = %i[
    total_heap_size
    total_heap_size_executable
    total_physical_size
    total_available_size
    total_global_handles_size
    used_global_handles_size
    used_heap_size
    heap_size_limit
    malloced_memory
    external_memory
    peak_malloced_memory
    number_of_native_contexts
    number_of_detached_contexts
  ].freeze

  def test_heap_stats_carry_mini_racers_names_in_its_order_as_integers
    stats = MiniRacer::Context.new.heap_stats

    assert_equal STATS, stats.keys
    assert_equal [Integer], stats.values.map(&:class).uniq
  end

  # mini_racer sends a heap snapshot without touching the exception a
  # callback raised, so a script that rethrows it after one still raises it.
  def test_a_nested_heap_snapshot_leaves_a_callbacks_exception_to_the_script
    context = MiniRacer::Context.new
    context.attach("boom", proc { raise ArgumentError, "kaboom" })
    context.attach("snap", proc { context.heap_snapshot && nil })

    error = assert_raises(ArgumentError) do
      context.eval("var saved; try { boom() } catch (e) { saved = e }; snap(); throw saved")
    end
    assert_equal "kaboom", error.message
  end
end
