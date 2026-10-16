# frozen_string_literal: true

require "test_helper"
require_relative "../diff/ledger"

class DiffLedgerTest < Minitest::Test
  REF = [["raise", "MiniRacer::RuntimeError", ["string", "UTF-8", "text", "SyntaxError: old text"]]].freeze
  OURS = [["raise", "MiniRacer::RuntimeError", ["string", "UTF-8", "text", "SyntaxError: new text"]]].freeze
  WORDING = { "id" => "syntax-wording", "compat" => "behavior:v8-version@ruby",
              "expected" => "old text", "actual" => "new text", }.freeze

  def result(id, klass)
    { "id" => id, "class" => klass }
  end

  def test_outcomes_are_same_accepted_crashed_or_diverged
    assert_equal "same", DiffLedger.classify(REF, REF, [])
    assert_equal "diverged", DiffLedger.classify(REF, OURS, [])
    assert_equal "accepted:syntax-wording", DiffLedger.classify(REF, OURS, [WORDING])
    assert_equal "crashed", DiffLedger.classify(REF, [["crash", "signal 11"]], [])
    assert_equal "crashed", DiffLedger.classify([["crash", "signal 6"]], [["crash", "signal 11"]], [])
  end

  def test_an_accepted_step_never_covers_another
    assert_equal "diverged", DiffLedger.classify(REF + [%w[int 2]], OURS + [%w[int 3]], [WORDING])
  end

  def test_the_signature_keeps_the_failure_and_blurs_digits
    bad_tag = ->(tag) { [["raise", "MiniRacer::RuntimeError", ["string", "UTF-8", "text", "bad tag: #{tag}"]]] }

    assert_equal DiffLedger.signature([%w[int 1]], bad_tag.call("0x41")),
                 DiffLedger.signature([%w[int 2]], bad_tag.call("0x6f"))
    refute_equal DiffLedger.signature([%w[int 1]], bad_tag.call("0x41")), DiffLedger.signature(REF, OURS)
  end

  def test_a_lost_reference_is_accepted_only_where_the_candidate_returns_the_same_object
    lost = { "id" => "lost", "compat" => "behavior:object-ids@ruby", "structure" => "lost-reference" }
    time = ["time", "0", 0, 0, false]
    text = %w[string UTF-8 text a]
    error = ["exception", "MiniRacer::ScriptError", text, [], ["nil"]]

    assert_equal "accepted:lost",
                 DiffLedger.classify([["array", [time, ["nil"], ["hash", []], ["nil"]]]],
                                     [["array", [time, ["again", time], ["hash", []], ["ref", 1]]]], [lost],)
    assert_equal "accepted:lost",
                 DiffLedger.classify([["array", [error, ["array", []]]]], [["array", [error, ["ref", 1]]]], [lost])
    assert_equal "diverged", DiffLedger.classify([["array", [%w[int 5], ["nil"]]]],
                                                 [["array", [%w[int 5], %w[int 5]]]], [lost],)
    assert_equal "diverged", DiffLedger.classify([["array", [["nil"], time, %w[int 5]]]],
                                                 [["array", [["nil"], time, %w[int 6]]]], [lost],)
  end

  def test_an_accepted_divergence_must_cite_a_compat_deviation
    compat = { "behavior:v8-version@ruby" => { "status" => "deviation" } }

    assert_empty DiffLedger.divergence_problems([WORDING], compat)
    assert_match "not a compat.toml deviation", DiffLedger.divergence_problems([WORDING], {}).first
    shaped = { "id" => "shaped", "compat" => "behavior:v8-version@ruby", "structure" => "lost-reference" }

    assert_empty DiffLedger.divergence_problems([shaped], compat)
    assert_match "unknown structure", DiffLedger.divergence_problems([shaped.merge("structure" => "x")], compat).first
  end

  def test_a_new_divergence_is_recorded_against_the_previous_milestone
    assert_equal "xfail:S3", DiffLedger.ledger_status(nil, "diverged")
    assert_equal "xfail:M2", DiffLedger.ledger_status("xfail:M2", "diverged")
    assert_equal "same", DiffLedger.ledger_status("xfail:M2", "same")
    assert_equal "crash:S3", DiffLedger.ledger_status("xfail:M2", "crashed")
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

  def test_a_known_gap_passes_but_a_new_crash_does_not
    assert_empty DiffLedger.ledger_problems({ "a" => { "status" => "xfail:S3" } }, [result("a", "diverged")], %w[S3])
    assert_empty DiffLedger.ledger_problems({ "a" => { "status" => "crash:S3" } }, [result("a", "crashed")], %w[S3])
    assert_match "a candidate crash needs crash:",
                 DiffLedger.ledger_problems({ "a" => { "status" => "xfail:S3" } }, [result("a", "crashed")],
                                            %w[S3],).first
  end
end
