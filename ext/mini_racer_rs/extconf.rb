# frozen_string_literal: true

require "mkmf"

if RUBY_ENGINE == "truffleruby"
  File.write("Makefile", dummy_makefile(__dir__).join)
  return
end

TARGETS = {
  %w[x86_64 linux] => "x86_64-unknown-linux-gnu",
  %w[aarch64 linux] => "aarch64-unknown-linux-gnu",
  %w[arm64 darwin] => "aarch64-apple-darwin",
  %w[aarch64 darwin] => "aarch64-apple-darwin",
  %w[x86_64 darwin] => "x86_64-apple-darwin",
}.freeze
MINIMUM_RUST = Gem::Version.new("1.91.0")

host_os = RbConfig::CONFIG["host_os"]
os = case host_os
     when /linux-musl/ then nil
     when /linux/ then "linux"
     when /darwin/ then "darwin"
     end
target = os && TARGETS[[RbConfig::CONFIG["host_cpu"], os]]
unless target
  abort "mini_racer_rs supports Linux (glibc) on x86_64 and aarch64 and macOS on arm64 and x86_64; " \
        "#{RbConfig::CONFIG["host_cpu"]}-#{host_os} is not supported"
end

rustc_version = `#{ENV.fetch("RUSTC", "rustc")} --version 2>/dev/null`[/\d+\.\d+\.\d+/]
if rustc_version.nil? || Gem::Version.new(rustc_version) < MINIMUM_RUST
  abort "mini_racer_rs needs Rust #{MINIMUM_RUST} or newer to build from source " \
        "(found #{rustc_version || "no rustc"}); install it from https://rustup.rs"
end

build_env = {}
unless ENV.key?("LIBCLANG_PATH")
  begin
    require "libclang"
    build_env["LIBCLANG_PATH"] = Libclang.libdir
    # The libclang gem ships no compiler builtin headers (stdarg.h); borrow GCC's.
    gcc_include = `gcc -print-file-name=include 2>/dev/null`.strip
    if os == "linux" && !ENV.key?("BINDGEN_EXTRA_CLANG_ARGS") && File.directory?(gcc_include)
      build_env["BINDGEN_EXTRA_CLANG_ARGS"] = "-I#{gcc_include}"
    end
  rescue LoadError
    libclang = find_executable("llvm-config") || Dir[
      "/usr/lib*/libclang*.so*",
      "/usr/lib*/llvm*/lib/libclang*.so*",
      "/usr/lib/*-linux-gnu/libclang*.so*",
      "/usr/lib/llvm-*/lib/libclang*.so*",
      "/Library/Developer/CommandLineTools/usr/lib/libclang.dylib",
      "/Applications/Xcode*.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/libclang.dylib",
      "/opt/homebrew/opt/llvm/lib/libclang.dylib",
      "/usr/local/opt/llvm/lib/libclang.dylib",
    ].first
    unless libclang
      abort "mini_racer_rs needs libclang to build from source: install clang " \
            "(for example `apt install libclang-dev`), run `gem install libclang`, or set LIBCLANG_PATH"
    end
  end
end

pinned = File.readlines(File.join(__dir__, "v8-archives.sha256"), chomp: true).to_h do |line|
  sha, file = line.split(/\s+/, 2)
  [file, sha]
end
unless %w[RUSTY_V8_ARCHIVE RUSTY_V8_MIRROR RUSTY_V8_ARCHIVE_SHA256].any? { ENV.key?(_1) }
  build_env["RUSTY_V8_ARCHIVE_SHA256"] = pinned.fetch("librusty_v8_release_#{target}.a.gz")
end

require "rb_sys/mkmf"

create_rust_makefile("mini_racer_rs/mini_racer_rs") do |r|
  r.extra_cargo_args += ["--locked"]
  r.env.merge!(build_env)
end
