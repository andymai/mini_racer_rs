# frozen_string_literal: true

require "fileutils"
require "rubygems/package"
require "tomlrb"

# Native gems carry one extension per Ruby minor, under
# lib/mini_racer_rs/<minor>/, which the loader picks by RUBY_VERSION.
module Package
  ROOT = File.expand_path("..", __dir__)
  PLATFORMS = %w[
    aarch64-linux-gnu aarch64-linux-musl arm64-darwin x86_64-darwin x86_64-linux-gnu x86_64-linux-musl
  ].freeze
  # The source gem's Rust sources and build files stay out of native gems.
  SOURCE_ONLY = %r{\A(?:Cargo\.(?:toml|lock)|crates/|ext/)}

  module_function

  def rubies
    Tomlrb.load_file(File.join(ROOT, "tools/versions.toml")).fetch("release").fetch("rubies")
  end

  # A copy each time: Gem::Specification.load caches its result, and a native
  # gem edits the spec. The gemspec's globs are relative to the root.
  def gemspec
    Dir.chdir(ROOT) { Gem::Specification.load(File.join(ROOT, "mini_racer_rs.gemspec")) }.dup
  end

  def dlext(platform)
    platform.end_with?("darwin") ? "bundle" : "so"
  end

  def upper_bound(minor)
    major, rest = minor.split(".").map(&:to_i)
    "< #{major}.#{rest + 1}.dev"
  end

  # The extension `rake compile` built for this Ruby, copied to where the
  # native gem task for `platform` looks for it.
  def stage(platform, native_dir)
    check_platform(platform)
    minor = RUBY_VERSION[/\A\d+\.\d+/]
    built = File.join(ROOT, "lib/mini_racer_rs/mini_racer_rs.#{dlext(platform)}")
    abort "#{built} not found; run rake compile" unless File.file?(built)
    dest = File.join(native_dir, platform, minor)
    FileUtils.mkdir_p(dest)
    FileUtils.cp(built, dest)
    File.join(dest, File.basename(built))
  end

  def native_spec(platform, extensions)
    spec = gemspec
    spec.platform = Gem::Platform.new(platform)
    spec.extensions = []
    spec.dependencies.reject! { |dependency| dependency.name == "rb_sys" }
    minors = extensions.keys.sort_by { |minor| Gem::Version.new(minor) }
    spec.required_ruby_version = [">= #{minors.first}", upper_bound(minors.last)]
    spec.files = spec.files.grep_v(SOURCE_ONLY) + extensions.values
    spec
  end

  def check_platform(platform)
    abort "unknown platform #{platform}; one of #{PLATFORMS.join(", ")}" unless PLATFORMS.include?(platform)
  end

  # The extension for each Ruby in tools/versions.toml, and no other.
  def staged(platform, native_dir)
    found = rubies.to_h do |minor|
      path = File.join(native_dir, platform, minor, "mini_racer_rs.#{dlext(platform)}")
      abort "#{path} not found; every Ruby in tools/versions.toml needs its extension" unless File.file?(path)
      [minor, path]
    end
    extra = Dir.children(File.join(native_dir, platform)) - found.keys
    if extra.any?
      abort "#{native_dir}/#{platform} has extensions for Rubies not in tools/versions.toml: #{extra.join(", ")}"
    end
    found
  end

  def copy(from, tree, file)
    FileUtils.mkdir_p(File.join(tree, File.dirname(file)))
    FileUtils.cp(from, File.join(tree, file))
  end

  # The gem's files without its sources, plus the extensions, laid out under
  # tmp/package/<platform> so no local build leaks in. Returns the tree and
  # each Ruby minor's extension path inside it.
  def tree(platform, extensions)
    root = File.join(ROOT, "tmp/package", platform)
    FileUtils.rm_rf(root)
    gemspec.files.grep_v(SOURCE_ONLY).select { |file| File.file?(File.join(ROOT, file)) }
           .each { |file| copy(File.join(ROOT, file), root, file) }
    files = extensions.to_h do |minor, path|
      file = "lib/mini_racer_rs/#{minor}/#{File.basename(path)}"
      copy(path, root, file)
      [minor, file]
    end
    [root, files]
  end

  # Builds the native gem for `platform` from the extensions staged under
  # `native_dir` into `out`.
  def native(platform, native_dir, out)
    check_platform(platform)
    root, files = tree(platform, staged(platform, native_dir))
    built = Dir.chdir(root) { Gem::Package.build(native_spec(platform, files)) }
    FileUtils.mkdir_p(out)
    FileUtils.mv(File.join(root, built), File.join(out, built))
    File.join(out, built)
  end

  def source(out)
    built = Dir.chdir(ROOT) { Gem::Package.build(gemspec) }
    FileUtils.mkdir_p(out)
    FileUtils.mv(File.join(ROOT, built), File.join(out, built))
    File.join(out, built)
  end
end

namespace :package do
  native_dir = ENV.fetch("NATIVE_DIR", File.join(Package::ROOT, "tmp/native"))
  out = ENV.fetch("PKG_DIR", File.join(Package::ROOT, "pkg"))

  desc "Copy the compiled extension to NATIVE_DIR/PLATFORM/<ruby minor>/ for package:native"
  task :stage do
    puts Package.stage(ENV.fetch("PLATFORM") { abort "PLATFORM=<gem platform> is required" }, native_dir)
  end

  desc "Build the native gem for PLATFORM from the extensions staged in NATIVE_DIR (default tmp/native)"
  task :native do
    puts Package.native(ENV.fetch("PLATFORM") { abort "PLATFORM=<gem platform> is required" }, native_dir, out)
  end

  desc "Build the source gem into PKG_DIR (default pkg)"
  task :source do
    puts Package.source(out)
  end
end
