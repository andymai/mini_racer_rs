# frozen_string_literal: true

require "test_helper"
require "open3"
require "rbconfig"

# Each case under fork/ runs in a process of its own, since V8's platform mode
# is fixed for a process at its first context.
class ForkTest < Minitest::Test
  # A deadlock can hold the GVL, so a child's own watchdog thread may never
  # run; the case is killed from here instead.
  TIMEOUT = 60

  def run_case(path)
    Open3.popen3(RbConfig.ruby, path) do |stdin, stdout, stderr, waiter|
      stdin.close
      out = Thread.new { stdout.read }
      err = Thread.new { stderr.read }
      unless waiter.join(TIMEOUT)
        Process.kill(:KILL, waiter.pid)
        waiter.join
      end
      [out.value, err.value, waiter.value]
    end
  end

  Dir[File.join(__dir__, "fork", "*_case.rb")].each do |path|
    define_method("test_#{File.basename(path, "_case.rb")}") do
      skip "fork needs CRuby" unless RUBY_ENGINE == "ruby" && Process.respond_to?(:fork)
      stdout, stderr, status = run_case(path)

      assert_predicate status, :success?, "status #{status.inspect}\nstdout:\n#{stdout}\nstderr:\n#{stderr}"
    end
  end
end
