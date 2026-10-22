# frozen_string_literal: true

require "test_helper"
require_relative "../../tools/compat/check"
require_relative "../../tools/compat/render"

class CompatExtraTest < Minitest::Test
  EVAL = "imethod:MiniRacer::Context#eval"
  RS = "module:MiniRacerRs"
  VERSION = "const:MiniRacerRs::VERSION"
  REGISTER = "smethod:MiniRacerRs.register_source_map"

  def problems(extra, present:, engine: "ruby")
    CompatCheck.problems(CompatCheck::Inputs.new(ledger: { "ids" => { EVAL => done }, "extra" => extra },
                                                 surface: { "ids" => [EVAL] },
                                                 candidate: { "engine" => engine, "ids" => [EVAL],
                                                              "extra_ids" => present, },
                                                 milestones: [],))
  end

  def done
    { "status" => "done" }
  end

  def ledger
    { RS => done, VERSION => done, "#{REGISTER}@ruby" => done }
  end

  def test_additions_recorded_per_engine_have_no_problems
    assert_empty problems(ledger, present: [RS, VERSION, REGISTER])
    assert_empty problems(ledger, present: [RS, VERSION], engine: "truffleruby")
  end

  def test_an_addition_without_an_entry_is_missing
    assert_equal ["MISSING_EXTRA #{REGISTER}: mini_racer_rs has it on ruby; compat.toml [extra] does not"],
                 problems(ledger.except("#{REGISTER}@ruby"), present: [RS, VERSION, REGISTER])
  end

  def test_an_entry_the_gem_lacks_on_its_engine_is_stale
    assert_equal ["STALE_EXTRA #{REGISTER}: not in mini_racer_rs on ruby"], problems(ledger, present: [RS, VERSION])
    assert_equal ["STALE_EXTRA #{VERSION}: not in mini_racer_rs on truffleruby"],
                 problems(ledger, present: [RS], engine: "truffleruby")
  end

  def test_extra_holds_only_mini_racer_rs_ids
    %w[smethod:MiniRacer::Platform.set_flags! module:MiniRacerRsX behavior:timeout@ruby].each do |id|
      assert_equal ["MALFORMED #{id}: [extra] holds only MiniRacerRs ids"],
                   problems({ RS => done, id => done }, present: [RS])
    end
  end

  def test_entries_are_tables_with_a_known_status_and_nothing_else
    assert_equal ["MALFORMED #{VERSION}: an [extra] entry is a table with a status"],
                 problems({ RS => done, VERSION => "done" }, present: [RS, VERSION])
    assert_equal ["MALFORMED #{VERSION}: status \"todo:M1\""],
                 problems({ RS => done, VERSION => { "status" => "todo:M1" } }, present: [RS, VERSION])
    assert_equal ["MALFORMED #{VERSION}: unknown keys reason"],
                 problems({ RS => done, VERSION => done.merge("reason" => "x") }, present: [RS, VERSION])
    assert_equal ["MALFORMED #{VERSION}@jruby: unknown engine jruby"],
                 problems({ RS => done, "#{VERSION}@jruby" => done }, present: [RS])
  end

  def test_additions_render_per_engine
    extra = { RS => done, "#{REGISTER}@ruby" => done, "module:MiniRacerRs::ForkHook@ruby" => { "status" => "internal" } }
    lines = CompatRender.additions(extra)

    assert_includes lines, "| `#{REGISTER}` | done | n/a |"
    assert_includes lines, "| `module:MiniRacerRs::ForkHook` | internal | n/a |"
    assert_includes lines, "| `#{RS}` | done | done |"
    assert_empty CompatRender.additions({})
  end
end
