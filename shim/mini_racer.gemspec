# frozen_string_literal: true

version_rb = File.read(File.expand_path("../lib/mini_racer_rs/version.rb", __dir__))
mini_racer_rs_version = version_rb[/VERSION = "([^"]+)"/, 1] or
  raise "VERSION not found in lib/mini_racer_rs/version.rb"

Gem::Specification.new do |spec|
  spec.name = "mini_racer"
  spec.version = "0.22.1"
  spec.authors = ["Andy Aragon"]
  spec.summary = "mini_racer_rs compatibility shim"
  spec.homepage = "https://github.com/andymai/mini_racer_rs"
  spec.license = "MIT"
  spec.required_ruby_version = ">= 3.3"
  spec.files = []
  spec.metadata = {
    "mini_racer_rs_shim" => "true",
    "rubygems_mfa_required" => "true",
  }
  spec.add_dependency "mini_racer_rs", mini_racer_rs_version
end
