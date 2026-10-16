# frozen_string_literal: true

require "test_helper"
require_relative "../../lib/mini_racer_rs/version"

class ExecjsAutodetectTest < Minitest::Test
  include RubyProcess

  EXECJS = File.expand_path("../../vendor/execjs/lib", __dir__)
  PROBE = 'require "execjs"; print ExecJS.runtime.name, "|", Gem.loaded_specs.key?("mini_racer"), "|", ' \
          'defined?(MiniRacerRs::VERSION) ? MiniRacerRs::VERSION : "none"'

  # ExecJS tries GraalJS before MiniRacer, and TruffleRuby provides GraalJS.
  def test_execjs_picks_mini_racer_rs_when_no_runtime_is_named
    skip "ExecJS prefers GraalJS on TruffleRuby" if RUBY_ENGINE == "truffleruby"

    assert_path_exists File.join(EXECJS, "execjs.rb"), "vendor/execjs is empty; run `git submodule update --init`"

    out, err, status = run_ruby(PROBE, env: { "EXECJS_RUNTIME" => nil }, load_path: [EXECJS])

    assert_predicate status, :success?, err
    assert_equal ["mini_racer (V8)", "false", MiniRacerRs::VERSION], out.split("|")
  end
end
