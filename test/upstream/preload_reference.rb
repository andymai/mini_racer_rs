# frozen_string_literal: true

# Reference mode: the real mini_racer 0.22.1 from tools/reference runs upstream's
# suite. This asserts that is what loaded, so reference results are upstream's own.

require "mini_racer"
spec = Gem.loaded_specs["mini_racer"]
unless spec && spec.version == Gem::Version.new("0.22.1") && !defined?(MiniRacerRs)
  warn "upstream harness: reference mode needs the real mini_racer 0.22.1 (got #{spec&.version.inspect})"
  exit!(97)
end

if ENV["MINI_RACER_RS_UPSTREAM_REPORT"] && ENV["MINI_RACER_RS_UPSTREAM_PARENT"] == Process.ppid.to_s
  require_relative "reporter"
end
