# frozen_string_literal: true

require "test_helper"
require_relative "../../tools/compat/render"

class CompatRenderTest < Minitest::Test
  EVAL = "imethod:MiniRacer::Context#eval"
  LOADER = "module:MiniRacer::Loader@ruby"
  ISOLATE = "class:MiniRacer::Isolate < Object@truffleruby"
  V8 = "behavior:v8-version@ruby"

  def test_merge_suffixes_ids_found_on_one_engine
    merged = CompatSurface.merge({ "ruby" => [EVAL, "module:MiniRacer::Loader"],
                                   "truffleruby" => [EVAL, "class:MiniRacer::Isolate < Object"], })

    assert_equal [ISOLATE, EVAL, LOADER].sort, merged
  end

  def test_counts_include_behavior_deviations_for_their_engine
    entries = { EVAL => { "status" => "done" }, V8 => { "status" => "deviation", "reason" => "x" } }

    assert_equal ["| CRuby | 1 | 0 | 1 |", "| TruffleRuby | 1 | 0 | 0 |"],
                 CompatRender.counts(entries, { "ids" => [EVAL] })
  end

  def test_the_readme_region_needs_both_markers
    assert_raises(ArgumentError) { CompatRender.readme_region("# x\n", {}, { "ids" => [] }) }
  end
end
