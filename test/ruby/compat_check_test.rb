# frozen_string_literal: true

require "test_helper"
require_relative "../../tools/compat/check"

class CompatCheckTest < Minitest::Test
  EVAL = "imethod:MiniRacer::Context#eval"
  STOP = "imethod:MiniRacer::Context#stop"
  SNAPSHOT = "imethod:MiniRacer::Context#heap_snapshot@ruby"
  WRITE = "imethod:MiniRacer::Context#write_heap_snapshot"
  LOADER = "module:MiniRacer::Loader@ruby"
  ISOLATE = "class:MiniRacer::Isolate < Object@truffleruby"
  TIMEOUT = "kwarg:MiniRacer::Context#initialize:timeout"
  V8 = "behavior:v8-version@ruby"

  def problems(entries, ids: [EVAL], present: [EVAL], engine: "ruby", **)
    CompatCheck.problems(CompatCheck::Inputs.new(ledger: { "ids" => entries }, surface: { "ids" => ids },
                                                 candidate: { "engine" => engine, "ids" => present },
                                                 milestones: %w[S2 M1 M3], **,))
  end

  def kinds(found)
    found.map { |line| line.split.first }
  end

  def done
    { "status" => "done" }
  end

  def test_a_consistent_ledger_has_no_problems
    assert_empty problems({ EVAL => done })
  end

  def test_surface_without_an_entry_is_missing
    assert_equal %w[MISSING], kinds(problems({}))
  end

  def test_an_entry_outside_the_surface_is_stale_unless_it_is_a_behavior
    entries = { EVAL => done, STOP => done, V8 => { "status" => "deviation", "reason" => "V8 15.2" } }

    assert_equal ["STALE #{STOP}: not in compat/surface.json"], problems(entries)
  end

  def test_statuses_reasons_and_dates_are_validated
    found = problems({ EVAL => { "status" => "deviation", "approved" => "2026-13-40" } }, present: [])

    assert_includes found, "MALFORMED #{EVAL}: a deviation needs a reason"
    assert_includes found, "MALFORMED #{EVAL}: approved must be YYYY-MM-DD"
    assert_includes problems({ EVAL => { "status" => "finished" } }), "MALFORMED #{EVAL}: status \"finished\""
    assert_includes problems({ EVAL => { "status" => "done", "reason" => "x" } }),
                    "MALFORMED #{EVAL}: only a deviation has a reason"
  end

  def test_an_entry_must_be_a_table
    assert_equal ["MALFORMED #{EVAL}: an entry is a table with a status"], problems({ EVAL => "done" })
  end

  def test_a_behavior_id_is_a_deviation
    assert_includes problems({ EVAL => done, V8 => done }), "MALFORMED #{V8}: a behavior id is always a deviation"
  end

  def test_a_milestone_must_be_declared
    assert_includes problems({ EVAL => { "status" => "todo:M10" } }, present: []), "UNKNOWN_MILESTONE #{EVAL}: M10"
  end

  def test_done_needs_the_method
    assert_equal %w[CLAIMED_DONE_BUT_ABSENT], kinds(problems({ EVAL => done }, present: []))
  end

  def test_a_planned_method_that_exists_must_be_marked_done
    assert_empty problems({ EVAL => { "status" => "todo:M1" } }, present: [])
    assert_equal %w[TODO_BUT_IMPLEMENTED], kinds(problems({ EVAL => { "status" => "todo:M1" } }))
  end

  def test_a_composition_waits_for_what_it_calls
    write = { "status" => "todo:M3", "depends_on" => [SNAPSHOT] }
    ids = [WRITE, SNAPSHOT]
    both = [WRITE, "imethod:MiniRacer::Context#heap_snapshot"]

    assert_empty problems({ WRITE => write, SNAPSHOT => { "status" => "todo:M3" } }, ids: ids, present: [WRITE])
    assert_equal %w[TODO_BUT_IMPLEMENTED],
                 kinds(problems({ WRITE => write, SNAPSHOT => done }, ids: ids, present: both))
    assert_includes problems({ WRITE => done.merge("depends_on" => [SNAPSHOT]), SNAPSHOT => { "status" => "todo:M3" } },
                             ids: ids, present: [WRITE],),
                    "MALFORMED #{WRITE}: done, but #{SNAPSHOT} is todo:M3"
  end

  def test_a_surface_deviation_means_absent
    entries = { EVAL => done, LOADER => { "status" => "deviation", "reason" => "x" } }

    assert_empty problems(entries, ids: [EVAL, LOADER])
    assert_equal %w[DEVIATION_BUT_PRESENT],
                 kinds(problems(entries, ids: [EVAL, LOADER], present: [EVAL, "module:MiniRacer::Loader"]))
  end

  def test_surface_that_mini_racer_lacks_is_extra
    assert_equal ["EXTRA_SURFACE #{STOP}: mini_racer 0.22.1 has no such ruby surface"],
                 problems({ EVAL => done }, present: [EVAL, STOP])
  end

  def test_engine_specific_ids_apply_only_to_their_engine
    entries = { EVAL => done, LOADER => { "status" => "deviation", "reason" => "x" }, ISOLATE => done }
    ids = [EVAL, LOADER, ISOLATE]

    assert_empty problems(entries, ids: ids)
    assert_empty problems(entries, ids: ids, present: [EVAL, "class:MiniRacer::Isolate < Object"],
                                   engine: "truffleruby",)
  end

  def test_truffleruby_must_match_the_reference_whatever_the_cruby_status
    entries = { EVAL => { "status" => "deviation", "reason" => "x" }, ISOLATE => { "status" => "todo:M1" } }
    found = problems(entries, ids: [EVAL, ISOLATE], present: [], engine: "truffleruby")

    assert_includes found, "CLAIMED_DONE_BUT_ABSENT #{EVAL}: upstream's backend provides it"
    assert_includes found, "MALFORMED #{ISOLATE}: TruffleRuby-only surface comes from upstream's backend and is done"
  end

  def test_keywords_are_checked_on_every_engine
    assert_equal %w[CLAIMED_DONE_BUT_ABSENT], kinds(problems({ TIMEOUT => done }, ids: [TIMEOUT], present: []))
    assert_equal %w[TODO_BUT_IMPLEMENTED],
                 kinds(problems({ TIMEOUT => { "status" => "todo:S2" } }, ids: [TIMEOUT], present: [TIMEOUT]))
  end
end
