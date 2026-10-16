# frozen_string_literal: true

require "test_helper"
require "tmpdir"
require_relative "../upstream/ledger"

class LedgerTest < Minitest::Test
  ID = "test/mini_racer_test.rb#MiniRacerTest#test_eval"
  LINUX = "ruby-3.4-linux-x86_64"
  MAC = "ruby-3.4-macos-arm64"

  def test_dump_round_trips_integers_in_overrides
    ledger = { ID => { "status" => "pass", "overrides" => { MAC => { "status" => "crash:S2", "timeout_s" => 20 } } } }
    Dir.mktmpdir do |dir|
      path = File.join(dir, "ledger.toml")
      File.write(path, UpstreamLedger.dump(ledger, [MAC, LINUX]))

      assert_equal ledger, UpstreamLedger.load(path)
      assert_equal [LINUX, MAC], UpstreamLedger.known_legs(path)
    end
  end

  def test_an_xfail_that_starts_crashing_needs_a_ledger_update
    assert_match "expected xfail:M1, got crash", UpstreamLedger.verdict("xfail:M1", "crash")
    assert_match "got timeout", UpstreamLedger.verdict("xfail:M1", "timeout")
    assert_nil UpstreamLedger.verdict("xfail:M1", "error")
  end

  def test_a_crash_entry_accepts_any_failure
    assert_nil UpstreamLedger.verdict("crash:S2", "timeout")
    assert_nil UpstreamLedger.verdict("crash:S2", "fail")
    assert_match "ratchet", UpstreamLedger.verdict("crash:S2", "pass")
  end

  def test_effective_override_status_is_validated
    entry = { "status" => "pass", "overrides" => { MAC => { "status" => "xfail:M10" } } }

    assert_match "undeclared milestone", UpstreamLedger.malformed(ID, entry, %w[M1]).first
  end

  def test_the_legs_list_takes_the_layout_taplo_picks
    short = UpstreamLedger.dump({}, %w[ruby-3.4-linux-x86_64])
    long = UpstreamLedger.dump({}, (1..8).map { |n| "ruby-3.#{n}-linux-x86_64" })

    assert_includes short, %(legs = ["ruby-3.4-linux-x86_64"])
    assert_includes long, %(legs = [\n  "ruby-3.1-linux-x86_64",\n)
  end
end
