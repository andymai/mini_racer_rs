# frozen_string_literal: true

require "test_helper"

# Every ```ruby check block in the README runs as written, in its own process,
# and each line ending in `# => value` must produce exactly that value, so the
# examples a reader copies stay true.
class ReadmeTest < Minitest::Test
  include RubyProcess

  README = File.expand_path("../../README.md", __dir__)
  EXECJS = File.expand_path("../../vendor/execjs/lib", __dir__)
  EXPECTATION = /\A(?<indent>\s*)(?<expression>\S.*?)\s+# => (?<expected>.+)\z/

  # The block is evaluated as README.md at its own line, so an expectation
  # that fails and an error Ruby raises both name the README's line.
  RUNNER = <<~RUBY
    def readme_expect(actual, expected, line)
      return if actual.eql?(expected)

      abort "README.md:\#{line}: expected \#{expected.inspect}, got \#{actual.inspect}"
    end
    eval(ENV.fetch("README_BLOCK"), TOPLEVEL_BINDING, "README.md", Integer(ENV.fetch("README_LINE")))
  RUBY

  # Each block with the README line its first line of code is on.
  def self.blocks
    lines = File.readlines(README)
    starts = lines.each_index.select { |index| lines[index].chomp == "```ruby check" }
    starts.map do |start|
      length = lines.drop(start + 1).index { |line| line.chomp == "```" }
      [lines[start + 1, length].join, start + 2]
    end
  end

  def self.instrument(source, first_line)
    source.lines.each_with_index.map do |line, index|
      match = EXPECTATION.match(line.chomp)
      next line unless match

      "#{match[:indent]}readme_expect((#{match[:expression]}), (#{match[:expected]}), #{first_line + index})\n"
    end.join
  end

  def test_the_readme_has_checked_blocks
    refute_empty self.class.blocks
  end

  blocks.each_with_index do |(source, first_line), index|
    define_method("test_readme_block_#{index + 1}") do
      skip "the TruffleRuby backend has its own runtime" unless RUBY_ENGINE == "ruby"
      env = { "README_BLOCK" => self.class.instrument(source, first_line), "README_LINE" => first_line.to_s,
              "EXECJS_RUNTIME" => nil, }
      out, err, status = run_ruby(RUNNER, env:, load_path: [EXECJS])

      assert_predicate status, :success?, "README block at line #{first_line}:\n#{err}#{out}"
    end
  end
end
