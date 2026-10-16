# frozen_string_literal: true

require "test_helper"
require "tempfile"
require_relative "../../tools/compat/check"
require_relative "../../tools/compat/render"

class CompatTest < Minitest::Test
  EVAL = "imethod:MiniRacer::Context#eval"
  STOP = "imethod:MiniRacer::Context#stop"
  LOADER = "module:MiniRacer::Loader@ruby"
  ISOLATE = "class:MiniRacer::Isolate < Object@truffleruby"
  TIMEOUT = "kwarg:MiniRacer::Context#initialize:timeout"

  def problems(entries, ids: [EVAL], present: [EVAL], stubs: [], engine: "ruby")
    CompatCheck.problems(ledger: { "ids" => entries }, surface: { "ids" => ids },
                         candidate: { "engine" => engine, "ids" => present, "stubs" => stubs },
                         engine: engine, milestones: %w[S2 M1],)
  end

  def kinds(found)
    found.map { |line| line.split.first }
  end

  def test_a_consistent_ledger_has_no_problems
    assert_empty problems({ EVAL => { "status" => "done" } })
  end

  def test_surface_without_an_entry_is_missing
    assert_equal %w[MISSING], kinds(problems({}, present: []).reject { _1.start_with?("EXTRA") })
  end

  def test_an_entry_outside_the_surface_is_stale_unless_it_is_a_behavior
    entries = { EVAL => { "status" => "done" }, STOP => { "status" => "done" },
                "behavior:v8-version" => { "status" => "deviation", "reason" => "V8 15.2" }, }

    assert_equal ["STALE #{STOP}: not in compat/surface.json"], problems(entries)
  end

  def test_statuses_reasons_and_dates_are_validated
    found = problems({ EVAL => { "status" => "deviation", "approved" => "2026-13-40" } })

    assert_includes found, "MALFORMED #{EVAL}: a deviation needs a reason"
    assert_includes found, "MALFORMED #{EVAL}: approved must be YYYY-MM-DD"
    assert_includes problems({ EVAL => { "status" => "finished" } }), "MALFORMED #{EVAL}: status \"finished\""
    assert_includes problems({ EVAL => { "status" => "done", "reason" => "x" } }),
                    "MALFORMED #{EVAL}: only a deviation has a reason"
  end

  def test_a_milestone_must_be_declared
    assert_includes problems({ EVAL => { "status" => "todo:M10" } }, present: []), "UNKNOWN_MILESTONE #{EVAL}: M10"
  end

  def test_done_needs_a_real_method
    assert_equal %w[CLAIMED_DONE_BUT_ABSENT], kinds(problems({ EVAL => { "status" => "done" } }, present: []))
    assert_equal %w[CLAIMED_DONE_BUT_ABSENT], kinds(problems({ EVAL => { "status" => "done" } }, stubs: [EVAL]))
  end

  def test_a_planned_method_that_exists_must_be_marked_done
    assert_empty problems({ EVAL => { "status" => "todo:M1" } }, stubs: [EVAL])
    assert_equal %w[TODO_BUT_IMPLEMENTED], kinds(problems({ EVAL => { "status" => "todo:M1" } }))
  end

  def test_surface_that_mini_racer_lacks_is_extra
    assert_equal ["EXTRA_SURFACE #{STOP}: mini_racer 0.22.1 has no such ruby surface"],
                 problems({ EVAL => { "status" => "done" } }, present: [EVAL, STOP])
  end

  def test_engine_specific_ids_apply_only_to_their_engine
    entries = { EVAL => { "status" => "done" }, LOADER => { "status" => "deviation", "reason" => "x" },
                ISOLATE => { "status" => "done" }, }
    ids = [EVAL, LOADER, ISOLATE]

    assert_empty problems(entries, ids: ids)
    assert_empty problems(entries, ids: ids, present: [EVAL, "class:MiniRacer::Isolate < Object"],
                                   engine: "truffleruby",)
  end

  def test_truffleruby_must_match_the_reference_whatever_the_cruby_status
    entries = { EVAL => { "status" => "todo:M1" }, ISOLATE => { "status" => "todo:M1" } }
    found = problems(entries, ids: [EVAL, ISOLATE], present: [], engine: "truffleruby")

    assert_includes found, "CLAIMED_DONE_BUT_ABSENT #{EVAL}: upstream's backend provides it"
    assert_includes found, "MALFORMED #{ISOLATE}: TruffleRuby-only surface comes from upstream's backend and is done"
  end

  def test_native_keywords_are_left_to_the_upstream_suite_on_cruby
    entries = { TIMEOUT => { "status" => "done" } }

    assert_empty problems(entries, ids: [TIMEOUT], present: [])
    assert_equal %w[CLAIMED_DONE_BUT_ABSENT],
                 kinds(problems(entries, ids: [TIMEOUT], present: [], engine: "truffleruby"))
  end

  def test_merge_suffixes_ids_found_on_one_engine
    merged = CompatSurface.merge({ "ruby" => [EVAL, "module:MiniRacer::Loader"],
                                   "truffleruby" => [EVAL, "class:MiniRacer::Isolate < Object"], })

    assert_equal [ISOLATE, EVAL, LOADER].sort, merged
  end

  def test_generated_stubs_raise_not_implemented_and_skip_existing_methods
    entries = { STOP => { "status" => "todo:S2" }, EVAL => { "status" => "todo:M1" } }
    source = CompatRender.stubs(entries).gsub("MiniRacer::Context", "CompatTest::FakeContext")
                         .sub("module MiniRacerRs", "module CompatTest::FakeStubs")
    fake = Class.new { def eval = :real }
    self.class.const_set(:FakeContext, fake)
    self.class.const_set(:FakeStubs, Module.new)
    CompatTest::FakeStubs.const_set(:VERSION, "0.0.0")
    Tempfile.create(["stubs", ".rb"]) do |file|
      file.write(source)
      file.flush
      load file.path
    end

    assert_equal ["imethod:CompatTest::FakeContext#stop"], CompatTest::FakeStubs::STUBS
    assert_equal :real, fake.new.eval
    assert_raises(NotImplementedError) { fake.new.stop }
  ensure
    self.class.send(:remove_const, :FakeContext) if self.class.const_defined?(:FakeContext, false)
    self.class.send(:remove_const, :FakeStubs) if self.class.const_defined?(:FakeStubs, false)
  end

  def test_the_readme_region_needs_both_markers
    assert_raises(ArgumentError) { CompatRender.readme_region("# x\n", {}, { "ids" => [] }) }
  end
end
