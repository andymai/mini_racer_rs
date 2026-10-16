# frozen_string_literal: true

require "test_helper"
require "open3"
require "rbconfig"

# Each case under fork/ runs in a process of its own, since V8's platform mode
# is fixed for a process at its first context.
class ForkTest < Minitest::Test
  Dir[File.join(__dir__, "fork", "*_case.rb")].each do |path|
    define_method("test_#{File.basename(path, "_case.rb")}") do
      skip "fork needs CRuby" unless RUBY_ENGINE == "ruby" && Process.respond_to?(:fork)
      stdout, stderr, status = Open3.capture3(RbConfig.ruby, path)

      assert_predicate status, :success?, "status #{status.exitstatus}\nstdout:\n#{stdout}\nstderr:\n#{stderr}"
    end
  end
end
