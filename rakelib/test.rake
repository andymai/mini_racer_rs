# frozen_string_literal: true

require "minitest/test_task"

# test:prebuilt runs the same tests on an extension already in lib/, such as
# one copied from an installed release gem, without rebuilding it.
%i[test test:prebuilt].each do |name|
  Minitest::TestTask.create(name) do |t|
    t.libs << "lib" << "test/ruby"
    t.test_globs = ["test/ruby/**/*_test.rb"]
  end
end

desc "Compile the extension, then run the Ruby tests"
task test: :compile
