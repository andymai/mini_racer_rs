# frozen_string_literal: true

require_relative "lib/mini_racer_rs/version"

Gem::Specification.new do |spec|
  spec.name = "mini_racer_rs"
  spec.version = MiniRacerRs::VERSION
  spec.authors = ["Andy Aragon"]
  spec.email = ["hi@andymai.com"]

  spec.summary = "A Rust drop-in replacement for mini_racer 0.22.1"
  spec.description = "mini_racer_rs provides the same MiniRacer Ruby API as mini_racer 0.22.1 " \
                     "using Rust and V8 15.2. Releases provide precompiled native gems for supported " \
                     "Linux and macOS platforms, plus a source gem that builds with Rust and libclang. " \
                     "The project is not affiliated with the mini_racer maintainers."
  spec.homepage = "https://github.com/andymai/mini_racer_rs"
  spec.license = "MIT"
  spec.required_ruby_version = ">= 3.3"

  spec.metadata = {
    "homepage_uri" => spec.homepage,
    "bug_tracker_uri" => "#{spec.homepage}/issues",
    "rubygems_mfa_required" => "true",
  }

  spec.files = Dir[
    "AI-DISCLOSURE.md",
    "Cargo.lock",
    "Cargo.toml",
    "LICENSE",
    "README.md",
    "crates/*/Cargo.toml",
    "crates/*/build.rs",
    "crates/*/src/**/*.rs",
    "ext/mini_racer_rs/Cargo.toml",
    "ext/mini_racer_rs/build.rs",
    "ext/mini_racer_rs/extconf.rb",
    "ext/mini_racer_rs/src/**/*.rs",
    "ext/mini_racer_rs/v8-archives.sha256",
    "lib/**/*.rb",
    "licenses/*",
  ]
  spec.require_paths = ["lib"]
  spec.extensions = ["ext/mini_racer_rs/extconf.rb"]

  spec.add_dependency "rb_sys", "= 0.9.130"
end
