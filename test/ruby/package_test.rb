# frozen_string_literal: true

require "test_helper"
require "rake"
require "tmpdir"

load File.expand_path("../../rakelib/package.rake", __dir__) unless defined?(Package)

class PackageTest < Minitest::Test
  PLATFORM = "x86_64-linux-gnu"

  def stage(dir, minors)
    minors.each do |minor|
      path = File.join(dir, PLATFORM, minor)
      FileUtils.mkdir_p(path)
      File.write(File.join(path, "mini_racer_rs.so"), "extension for #{minor}")
    end
  end

  def test_a_native_gem_carries_one_extension_per_ruby_and_no_sources
    Dir.mktmpdir do |dir|
      stage(File.join(dir, "native"), Package.rubies)
      spec = Gem::Package.new(Package.native(PLATFORM, File.join(dir, "native"), File.join(dir, "pkg"))).spec
      oldest, newest = Package.rubies.minmax_by { |minor| Gem::Version.new(minor) }

      assert_equal PLATFORM, spec.platform.to_s
      assert_empty spec.extensions
      assert_empty(spec.runtime_dependencies.map(&:name))
      assert_equal Gem::Requirement.new(">= #{oldest}", Package.upper_bound(newest)), spec.required_ruby_version
      Package.rubies.each { |minor| assert_includes spec.files, "lib/mini_racer_rs/#{minor}/mini_racer_rs.so" }
      assert_empty spec.files.grep(Package::SOURCE_ONLY)
      %w[lib/mini_racer.rb lib/mini_racer_rs.rb LICENSE THIRD_PARTY_NOTICES.md].each do |file|
        assert_includes spec.files, file
      end
    end
  end

  def test_the_source_gem_keeps_its_build_after_a_native_gem
    Dir.mktmpdir do |dir|
      stage(File.join(dir, "native"), Package.rubies)
      Package.native(PLATFORM, File.join(dir, "native"), File.join(dir, "pkg"))
      spec = Package.gemspec

      assert_equal Gem::Platform::RUBY, spec.platform
      assert_equal ["ext/mini_racer_rs/extconf.rb"], spec.extensions
      assert_includes spec.runtime_dependencies.map(&:name), "rb_sys"
    end
  end

  def test_every_ruby_needs_its_extension
    Dir.mktmpdir do |dir|
      stage(File.join(dir, "native"), Package.rubies.drop(1))
      _, err = capture_subprocess_io do
        assert_raises(SystemExit) { Package.native(PLATFORM, File.join(dir, "native"), File.join(dir, "pkg")) }
      end

      assert_match(/#{Regexp.escape(Package.rubies.first)}.*not found/, err)
    end
  end

  def test_an_extension_for_an_unlisted_ruby_is_refused
    Dir.mktmpdir do |dir|
      stage(File.join(dir, "native"), Package.rubies + ["9.9"])
      _, err = capture_subprocess_io do
        assert_raises(SystemExit) { Package.native(PLATFORM, File.join(dir, "native"), File.join(dir, "pkg")) }
      end

      assert_match(%r{not in tools/versions\.toml: 9\.9}, err)
    end
  end
end
