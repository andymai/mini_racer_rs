# frozen_string_literal: true

require "test_helper"
require "mini_racer"

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

  def test_context_cannot_be_allocated_directly
    assert_raises(TypeError) { MiniRacer::Context.allocate }
  end
end
