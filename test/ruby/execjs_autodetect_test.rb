# frozen_string_literal: true

require "test_helper"
require "open3"
require "rbconfig"

class ExecjsAutodetectTest < Minitest::Test
  ROOT = File.expand_path("../..", __dir__)
  PROBE = 'require "execjs"; print ExecJS.runtime.name, "|", Gem.loaded_specs.key?("mini_racer"), "|", ' \
          'defined?(MiniRacerRs::VERSION) ? MiniRacerRs::VERSION : "none"'

  # ExecJS tries GraalJS before MiniRacer, and TruffleRuby provides GraalJS.
  def test_execjs_picks_mini_racer_rs_when_no_runtime_is_named
    skip "ExecJS prefers GraalJS on TruffleRuby" if RUBY_ENGINE == "truffleruby"

    out, status = Open3.capture2e({ "EXECJS_RUNTIME" => nil }, RbConfig.ruby,
                                  "-I#{File.join(ROOT, "vendor/execjs/lib")}", "-I#{File.join(ROOT, "lib")}",
                                  "-e", PROBE,)

    assert_predicate status, :success?, out
    assert_equal ["mini_racer (V8)", "false", MiniRacerRs::VERSION], out.split("|")
  end
end
