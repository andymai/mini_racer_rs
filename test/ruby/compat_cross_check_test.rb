# frozen_string_literal: true

require "test_helper"
require_relative "../../tools/compat/check"

class CompatCrossCheckTest < Minitest::Test
  EVAL = "imethod:MiniRacer::Context#eval"
  STOP = "imethod:MiniRacer::Context#stop"
  V8 = "behavior:v8-version@ruby"

  def problems(entries, **)
    CompatCheck.problems(CompatCheck::Inputs.new(ledger: { "ids" => entries }, surface: { "ids" => [EVAL] },
                                                 candidate: { "engine" => "ruby", "ids" => [EVAL] },
                                                 milestones: [], **,))
  end

  def done
    { "status" => "done" }
  end

  def test_an_upstream_ledger_may_cite_only_a_recorded_deviation
    upstream = { "mini_racer.status.toml" => {
      "t#A#ok" => { "status" => "deviation:#{V8}" },
      "t#A#typo" => { "status" => "pass", "overrides" => { "leg" => { "status" => "deviation:behavior:v8" } } },
    } }
    entries = { EVAL => done, V8 => { "status" => "deviation", "reason" => "V8 15.2" } }

    assert_equal ["UNKNOWN_DEVIATION mini_racer.status.toml: t#A#typo cites behavior:v8, " \
                  "which compat.toml does not record as a deviation"],
                 problems(entries, upstream_ledgers: upstream)
  end

  def test_a_fresh_reference_must_match_the_surface
    found = problems({ EVAL => done }, reference: { "ids" => [STOP] })

    assert_includes found, "SURFACE_DRIFT #{STOP}: mini_racer 0.22.2 has it on ruby; compat/surface.json does not"
    assert_includes found, "SURFACE_DRIFT #{EVAL}: in compat/surface.json but not in mini_racer 0.22.2 on ruby"
  end

  def test_a_scalar_entry_never_reaches_later_checks
    entries = { EVAL => { "status" => "todo:M1", "depends_on" => [STOP] }, STOP => "done" }

    assert_includes problems(entries), "MALFORMED #{STOP}: an entry is a table with a status"
  end

  def test_an_engine_suffix_must_name_an_engine
    entries = { EVAL => done, "behavior:platforms@rubyy" => { "status" => "deviation", "reason" => "x" } }

    assert_includes problems(entries), "MALFORMED behavior:platforms@rubyy: unknown engine rubyy"
  end
end
