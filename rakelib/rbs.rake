# frozen_string_literal: true

require "minitest/test_task"

desc "Validate the RBS signatures in sig/"
task "rbs:validate" do
  sh "rbs", "-I", "sig", "validate"
end

Minitest::TestTask.create("rbs:test") do |t|
  t.libs << "lib" << "test/ruby" << "test/rbs"
  t.test_globs = ["test/ruby/**/*_test.rb"]
  t.test_prelude = %(require "runtime_check")
end

desc "Compile the extension, then run the Ruby tests with their calls checked against sig/"
task "rbs:test" => :compile
