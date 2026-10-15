# frozen_string_literal: true

require "test_helper"
require_relative "../upstream/milestone_rules"

class LedgerMergeTest < Minitest::Test
  ID = "test/mini_racer_test.rb#MiniRacerTest#test_eval"
  LINUX = "ruby-3.4-linux-x86_64"
  MAC = "ruby-3.4-macos-arm64"

  def row(mode, status, leg: LINUX, id: ID, message: "")
    { "id" => id, "mode" => mode, "status" => status, "leg" => leg, "message" => message }
  end

  def merge(existing, rows, **)
    LedgerMerge.merge(existing, rows, **)
  end

  def merged(existing, rows, **) = merge(existing, rows, **).tests

  def test_tests_without_results_are_kept
    other = { "test/x.rb#X#test_y" => { "status" => "xfail:M3", "timeout_s" => 20 } }
    ledger = merged(other, [row("reference", "pass"), row("candidate", "pass")])

    assert_equal({ "status" => "xfail:M3", "timeout_s" => 20 }, ledger["test/x.rb#X#test_y"])
    assert_equal({ "status" => "pass" }, ledger[ID])
  end

  def test_a_regression_stays_pass_with_a_warning
    result = merge({ ID => { "status" => "pass" } }, [row("reference", "pass"), row("candidate", "fail")])

    assert_equal "pass", result.tests[ID]["status"]
    assert_match "kept pass", result.warnings.first
  end

  def test_an_upstream_skip_on_a_new_leg_is_not_a_regression
    rows = [row("reference", "skip", leg: MAC), row("candidate", "skip", leg: MAC)]
    result = merge({ ID => { "status" => "pass" } }, rows, legs: [LINUX])

    assert_empty result.warnings
    assert_equal "upstream-skip", result.tests[ID]["overrides"][MAC]["status"]
  end

  def test_allowing_regressions_records_them
    ledger = merged({ ID => { "status" => "pass" } }, [row("reference", "pass"), row("candidate", "fail")],
                    allow_regressions: true,)

    assert_equal "xfail:M1", ledger[ID]["status"]
  end

  def test_passing_where_upstream_fails_is_pass
    ledger = merged({}, [row("reference", "fail"), row("candidate", "pass")])

    assert_equal "pass", ledger[ID]["status"]
  end

  def test_failing_where_upstream_fails_is_upstream_fail_with_cause
    ledger = merged({}, [row("reference", "error", message: "boom"), row("candidate", "error")])

    assert_equal "upstream-fail", ledger[ID]["status"]
    assert_equal "mini_racer 0.22.1: error: boom", ledger[ID]["cause"]
  end

  def test_per_leg_milestones_and_timeouts_survive
    existing = { ID => { "status" => "pass", "overrides" => { MAC => { "status" => "xfail:M8", "timeout_s" => 20 } } } }
    rows = [row("reference", "pass"), row("candidate", "pass"),
            row("reference", "pass", leg: MAC), row("candidate", "fail", leg: MAC),]
    ledger = merged(existing, rows)

    assert_equal({ "status" => "xfail:M8", "timeout_s" => 20 }, ledger[ID]["overrides"][MAC])
  end

  def test_legs_without_results_keep_their_overrides
    existing = { ID => { "status" => "xfail:M1", "overrides" => { MAC => { "status" => "upstream-skip" } } } }
    ledger = merged(existing, [row("reference", "pass"), row("candidate", "fail")])

    assert_equal "upstream-skip", ledger[ID]["overrides"][MAC]["status"]
  end

  def test_a_partial_merge_leaves_other_legs_alone
    ledger = merged({ ID => { "status" => "xfail:M1" } }, [row("reference", "pass"), row("candidate", "pass")],
                    legs: [LINUX, MAC],)

    assert_equal "xfail:M1", UpstreamLedger.effective(ledger[ID], MAC)["status"]
    assert_equal "pass", UpstreamLedger.effective(ledger[ID], LINUX)["status"]
  end

  def test_a_full_merge_moves_the_base
    rows = [LINUX, MAC].flat_map { |leg| [row("reference", "pass", leg:), row("candidate", "pass", leg:)] }
    ledger = merged({ ID => { "status" => "xfail:M1" } }, rows, legs: [LINUX, MAC])

    assert_equal({ "status" => "pass" }, ledger[ID])
  end

  def test_merged_legs_join_the_known_legs
    result = merge({}, [row("reference", "pass", leg: MAC), row("candidate", "pass", leg: MAC)], legs: [LINUX])

    assert_equal [LINUX, MAC], result.legs
  end

  def test_prune_drops_legs_without_results
    existing = { ID => { "status" => "pass", "overrides" => { MAC => { "status" => "upstream-skip" } } } }
    result = merge(existing, [row("reference", "pass"), row("candidate", "pass")], legs: [LINUX, MAC], prune: true)

    assert_equal [LINUX], result.legs
    assert_equal({ "status" => "pass" }, result.tests[ID])
  end

  def test_prune_drops_tests_without_results
    ledger = merged({ "test/x.rb#X#gone" => { "status" => "pass" } },
                    [row("reference", "pass"), row("candidate", "pass")], prune: true,)

    refute ledger.key?("test/x.rb#X#gone")
  end
end
