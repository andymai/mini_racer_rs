# frozen_string_literal: true

require "test_helper"
require_relative "../diff/ledger"

class DiffLedgerTest < Minitest::Test
  REF = [["raise", "MiniRacer::RuntimeError", "SyntaxError: old text"]].freeze
  OURS = [["raise", "MiniRacer::RuntimeError", "SyntaxError: new text"]].freeze
  WORDING = { "id" => "syntax-wording", "compat" => "behavior:v8-version@ruby",
              "expected" => "old text", "actual" => "new text", }.freeze

  def result(id, klass)
    { "id" => id, "class" => klass }
  end

  def test_outcomes_are_same_accepted_or_diverged
    assert_equal "same", DiffLedger.classify(REF, REF, [])
    assert_equal "diverged", DiffLedger.classify(REF, OURS, [])
    assert_equal "accepted:syntax-wording", DiffLedger.classify(REF, OURS, [WORDING])
  end

  def test_an_accepted_divergence_must_cite_a_compat_deviation
    compat = { "behavior:v8-version@ruby" => { "status" => "deviation" } }

    assert_empty DiffLedger.divergence_problems([WORDING], compat)
    assert_match "not a compat.toml deviation", DiffLedger.divergence_problems([WORDING], {}).first
  end

  def test_a_new_divergence_is_recorded_against_the_previous_milestone
    assert_equal "xfail:S3", DiffLedger.ledger_status(nil, "diverged")
    assert_equal "xfail:M2", DiffLedger.ledger_status("xfail:M2", "diverged")
    assert_equal "same", DiffLedger.ledger_status("xfail:M2", "same")
  end

  def test_strict_mode_rejects_ratchets_unlisted_stale_and_undeclared
    ledger = { "a" => { "status" => "xfail:S3" }, "b" => { "status" => "same" }, "c" => { "status" => "xfail:M9" },
               "gone" => { "status" => "same" }, }
    results = [result("a", "same"), result("b", "diverged"), result("c", "diverged"), result("new", "same")]
    problems = DiffLedger.ledger_problems(ledger, results, %w[S3])

    assert_equal ["a: ratchet: xfail:S3 now same; update the ledger", "b: expected same, got diverged",
                  "c: undeclared milestone M9", "new: not in the ledger (same)",
                  "gone: in the ledger but did not run",],
                 problems
  end

  def test_a_known_gap_passes
    assert_empty DiffLedger.ledger_problems({ "a" => { "status" => "xfail:S3" } }, [result("a", "diverged")], %w[S3])
  end
end
