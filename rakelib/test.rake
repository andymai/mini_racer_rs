# frozen_string_literal: true

require "minitest/test_task"

Minitest::TestTask.create(:test) do |t|
  t.libs << "lib" << "test/ruby"
  t.test_globs = ["test/ruby/**/*_test.rb"]
end

desc "Compile the extension, then run the Ruby tests"
task test: :compile
