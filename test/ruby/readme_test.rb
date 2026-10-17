# frozen_string_literal: true

require "test_helper"

# Every ```ruby check block in the README runs as written, in its own process,
# and each line ending in `# => value` must produce that value, so the examples
# a reader copies stay true.
class ReadmeTest < Minitest::Test
  include RubyProcess

  README = File.expand_path("../../README.md", __dir__)
  EXECJS = File.expand_path("../../vendor/execjs/lib", __dir__)
  EXPECTATION = /\A(?<indent>\s*)(?<expression>\S.*?)\s+# => (?<expected>.+)\z/

  def self.blocks
    File.read(README).scan(/^```ruby check\n(.*?)^```$/m).flatten
  end

  # Lines keep their numbers, so a failure names the line within the block.
  def self.instrument(source)
    checked = source.lines.each_with_index.map do |line, index|
      match = EXPECTATION.match(line.chomp)
      next line unless match

      "#{match[:indent]}readme_expect((#{match[:expression]}), (#{match[:expected]}), #{index + 1})\n"
    end
    <<~RUBY + checked.join
      def readme_expect(actual, expected, line)
        return if actual == expected

        abort "line \#{line}: expected \#{expected.inspect}, got \#{actual.inspect}"
      end
    RUBY
  end

  def test_the_readme_has_checked_blocks
    refute_empty self.class.blocks
  end

  blocks.each_with_index do |source, index|
    define_method("test_readme_block_#{index + 1}") do
      skip "the TruffleRuby backend has its own runtime" unless RUBY_ENGINE == "ruby"
      out, err, status = run_ruby(self.class.instrument(source), load_path: [EXECJS])

      assert_predicate status, :success?, "README block #{index + 1}:\n#{err}#{out}"
    end
  end
end
