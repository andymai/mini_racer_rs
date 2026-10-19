# frozen_string_literal: true

require "test_helper"
require "open3"
require "rbconfig"

# Each case under stack/ runs in a process of its own: Ruby reads its stack
# sizes once at boot, and a stack that runs out under Rust frames can leave the
# process hung.
class StackTest < Minitest::Test
  TIMEOUT = 60
  # Ruby's smallest fiber and thread machine stacks, so a case reaches the end
  # of either in few levels.
  SMALL_STACKS = {
    "RUBY_FIBER_MACHINE_STACK_SIZE" => "131072",
    "RUBY_THREAD_MACHINE_STACK_SIZE" => "131072",
  }.freeze

  def run_case(path)
    Open3.popen3(SMALL_STACKS, RbConfig.ruby, path, pgroup: true) do |stdin, stdout, stderr, waiter|
      stdin.close
      out = Thread.new { stdout.read }
      err = Thread.new { stderr.read }
      unless waiter.join(TIMEOUT)
        Process.kill(:KILL, -waiter.pid)
        waiter.join
      end
      [out.value, err.value, waiter.value]
    end
  end

  Dir[File.join(__dir__, "stack", "*_case.rb")].each do |path|
    define_method("test_#{File.basename(path, "_case.rb")}") do
      skip "needs CRuby's machine stack checks" unless RUBY_ENGINE == "ruby"
      stdout, stderr, status = run_case(path)

      assert_predicate status, :success?, "status #{status.inspect}\nstdout:\n#{stdout}\nstderr:\n#{stderr}"
    end
  end
end
