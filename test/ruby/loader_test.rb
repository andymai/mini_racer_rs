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
