# frozen_string_literal: true

require "test_helper"
require "fileutils"
require "mini_racer_rs/version"
require "tmpdir"

class HarnessGuardTest < Minitest::Test
  PRELOAD = File.expand_path("../upstream/preload.rb", __dir__)

  def setup
    @dir = Dir.mktmpdir
    @lib = File.join(@dir, "lib")
    FileUtils.mkdir_p(File.join(@lib, "mini_racer"))
    File.write(File.join(@lib, "mini_racer.rb"), "FAKE_UPSTREAM = true\n")
    File.write(File.join(@lib, "mini_racer", "extra.rb"), "FAKE_UPSTREAM_EXTRA = true\n")
  end

  def teardown
    FileUtils.rm_rf(@dir)
  end

  def guarded(code)
    env = {
      "MINI_RACER_RS_UPSTREAM_LIB" => @lib,
      "RUBYOPT" => "#{ENV.fetch("RUBYOPT", "")} -r#{PRELOAD}".strip,
    }
    Open3.capture3(env, RbConfig.ruby, "-e", code)
  end

  def test_upstream_require_after_unshift_still_gets_mini_racer_rs
    out, err, status = guarded(<<~RUBY)
      $LOAD_PATH.unshift(#{@lib.inspect})
      require "mini_racer"
      puts defined?(FAKE_UPSTREAM).inspect, MiniRacerRs::VERSION
    RUBY

    assert_predicate status, :success?, err
    assert_equal "nil\n#{MiniRacerRs::VERSION}\n", out
  end

  def test_loading_an_upstream_file_trips_the_guard
    _, err, status = guarded("load #{File.join(@lib, "mini_racer.rb").inspect}")

    assert_equal 97, status.exitstatus
    assert_match "the suite must exercise mini_racer_rs", err
  end

  def test_requiring_an_upstream_file_by_path_trips_the_guard
    _, err, status = guarded("require #{File.join(@lib, "mini_racer", "extra").inspect}")

    assert_equal 97, status.exitstatus, err
  end

  def test_child_processes_inherit_the_guard
    code = %(system(RbConfig.ruby, "-e", "load '#{File.join(@lib, "mini_racer.rb")}'"); exit($?.exitstatus))
    _, err, status = guarded(code)

    assert_equal 97, status.exitstatus, err
  end
end
