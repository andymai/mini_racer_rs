# frozen_string_literal: true

require "test_helper"
require "fileutils"
require "mini_racer"
require "tmpdir"

class LoaderTest < Minitest::Test
  LIB = File.expand_path("../../lib", __dir__)

  def test_version_constants
    assert_equal "0.22.1", MiniRacer::VERSION
    assert_equal "~> 24.12.0.1", MiniRacer::LIBV8_NODE_VERSION
    assert_equal MiniRacerRs::VERSION, MiniRacerRs::NATIVE_VERSION
    assert_match(/\A15\.2\./, MiniRacerRs::V8_VERSION)
  end

  def test_features_load_from_this_gem
    %w[mini_racer.rb mini_racer/version.rb mini_racer_rs.rb mini_racer_rs/vocabulary.rb].each do |feature|
      path = $LOADED_FEATURES.find { _1.end_with?("/#{feature}") }

      assert path&.start_with?(LIB), "#{feature} loaded from #{path.inspect}"
    end
  end

  # A native gem holds one extension per Ruby minor under
  # lib/mini_racer_rs/<minor>/; the one for this Ruby wins over a source build.
  def test_the_extension_for_this_ruby_minor_wins
    skip "the TruffleRuby backend loads no extension" unless RUBY_ENGINE == "ruby"
    dlext = RbConfig::CONFIG["DLEXT"]
    minor = RUBY_VERSION[/\A\d+\.\d+/]
    Dir.mktmpdir do |dir|
      Dir.glob("**/*.rb", base: LIB).each do |file|
        FileUtils.mkdir_p(File.join(dir, File.dirname(file)))
        FileUtils.cp(File.join(LIB, file), File.join(dir, file))
      end
      FileUtils.mkdir_p(File.join(dir, "mini_racer_rs", minor))
      built = File.join(LIB, "mini_racer_rs", "mini_racer_rs.#{dlext}")
      File.symlink(built, File.join(dir, "mini_racer_rs", minor, "mini_racer_rs.#{dlext}"))
      File.write(File.join(dir, "mini_racer_rs", "mini_racer_rs.#{dlext}"), "not an extension")
      code = <<~RUBY
        require "mini_racer"
        puts $LOADED_FEATURES.find { _1.end_with?("mini_racer_rs.#{dlext}") }
        p MiniRacer::Context.new.eval("1 + 1")
      RUBY
      out, err, status = Open3.capture3(RbConfig.ruby, "-I", dir, "-e", code)

      assert_predicate status, :success?, err
      loaded, result = out.lines.map(&:chomp)

      assert_includes loaded, "/mini_racer_rs/#{minor}/"
      assert_equal "2", result
    end
  end

  def test_an_allocated_context_needs_initialize
    assert_raises(MiniRacer::InternalError) { MiniRacer::Context.allocate.eval("1") }
  end

  def test_context_can_be_subclassed
    subclass = Class.new(MiniRacer::Context) do
      attr_reader :label

      def initialize(label)
        super()
        @label = label
      end
    end
    context = subclass.new("mine")

    assert_instance_of subclass, context
    assert_equal "mine", context.label
    assert_equal 2, context.eval("1 + 1")
  end
end
