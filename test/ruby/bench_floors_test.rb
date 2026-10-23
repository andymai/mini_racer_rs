# frozen_string_literal: true

require "test_helper"
require_relative "../../bench/floors"

class BenchFloorsTest < Minitest::Test
  def result(summary = {}, serde = {})
    base = { "call round trip p50 (us)" => { "ratio" => 11.0, "note" => "" },
             "startup p50 (ms)" => { "ratio" => 1.2, "note" => "" }, }
    { "summary" => base.merge(summary), "serde" => { "js_to_ruby/return_10k_ints" => { "ratio" => 1.4 } }.merge(serde) }
  end

  def test_rows_at_their_floors_pass
    assert_empty BenchFloors.misses(result({ "startup p50 (ms)" => { "ratio" => 0.8, "note" => "noisy" },
                                             "eval round trip p50 (us)" => { "ratio" => 2.0, "note" => "" }, }))
  end

  def test_round_trips_keep_twice_the_reference
    misses = BenchFloors.misses(result({ "callback round trip p50 (us)" => { "ratio" => 1.9, "note" => "" } }))

    assert_equal({ "callback round trip p50 (us)" => "1.90x, floor 2.0x" }, misses)
    assert_in_delta 2.0, BenchFloors.floor_for("call in a timeout scope p50 (us)")
  end

  def test_a_serde_case_below_the_floor_fails
    misses = BenchFloors.misses(result({}, { "ruby_to_js/roundtrip_10k_ints" => { "ratio" => 0.79 } }))

    assert_equal ["serde ruby_to_js/roundtrip_10k_ints"], misses.keys
  end

  def test_tail_latency_and_rows_under_their_floor_are_not_gated
    assert_empty BenchFloors.misses(result({ "call round trip p99 (us)" => { "ratio" => 0.5, "note" => "noisy" },
                                             "call round trip p99.9 (us)" => { "ratio" => 0.4, "note" => "" },
                                             "idle CPU, 50 contexts (% of a core)" => { "ratio" => nil,
                                                                                        "note" => "under 1.0", }, }))
  end

  def test_a_row_without_a_ratio_fails
    misses = BenchFloors.misses(result({ "JS to Ruby (MB/s)" => { "ratio" => nil, "note" => "different output" } }))

    assert_equal({ "JS to Ruby (MB/s)" => "no ratio (different output)" }, misses)
  end

  def test_only_a_miss_in_every_run_is_confirmed
    slow = result({ "startup p50 (ms)" => { "ratio" => 0.7, "note" => "" } })
    slower = result({ "startup p50 (ms)" => { "ratio" => 0.6, "note" => "" } })

    assert_empty BenchFloors.confirmed([slow, result])
    assert_equal({ "startup p50 (ms)" => ["0.70x, floor 0.8x", "0.60x, floor 0.8x"] },
                 BenchFloors.confirmed([slow, slower]),)
  end
end
