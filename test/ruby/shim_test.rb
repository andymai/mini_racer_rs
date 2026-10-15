# frozen_string_literal: true

require "test_helper"
require "mini_racer_rs/version"

class ShimTest < Minitest::Test
  SHIM = File.expand_path("../../shim/mini_racer.gemspec", __dir__)

  def test_shim_satisfies_mini_racer_dependents
    spec = Gem::Specification.load(SHIM)

    assert_equal "mini_racer", spec.name
    assert_equal Gem::Version.new("0.22.1"), spec.version
    assert_equal "true", spec.metadata["mini_racer_rs_shim"]
    assert_empty spec.files
  end

  def test_shim_pins_this_version
    dependency = Gem::Specification.load(SHIM).runtime_dependencies.find { _1.name == "mini_racer_rs" }

    assert_equal Gem::Requirement.new(MiniRacerRs::VERSION), dependency.requirement
  end
end
