# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class VocabularyTest < Minitest::Test
  HIERARCHY = {
    MiniRacer::Error => StandardError,
    MiniRacer::ContextDisposedError => MiniRacer::Error,
    MiniRacer::PlatformAlreadyInitialized => MiniRacer::Error,
    MiniRacer::EvalError => MiniRacer::Error,
    MiniRacer::ParseError => MiniRacer::EvalError,
    MiniRacer::ScriptTerminatedError => MiniRacer::EvalError,
    MiniRacer::V8OutOfMemoryError => MiniRacer::EvalError,
    MiniRacer::RuntimeError => MiniRacer::EvalError,
    MiniRacer::ScriptError => MiniRacer::EvalError,
    MiniRacer::SnapshotError => MiniRacer::Error,
    MiniRacerRs::InternalError => MiniRacer::Error,
  }.freeze

  def test_error_hierarchy_matches_upstream
    HIERARCHY.each { |klass, parent| assert_equal parent, klass.superclass, klass.name }
  end

  def test_runtime_error_splits_javascript_frames
    error = MiniRacer::RuntimeError.new("Error: boom\n    at a (<eval>:1:7)\n    at <eval>:1:20")
    error.set_backtrace(["ruby.rb:1"])

    assert_equal "Error: boom", error.message
    assert_equal ["JavaScript at a (<eval>:1:7)", "JavaScript at <eval>:1:20", "ruby.rb:1"], error.backtrace
  end

  def test_binary_requires_a_string
    assert_equal "abc", MiniRacer::Binary.new("abc").data
    assert_raises(TypeError) { MiniRacer::Binary.new(1) }
  end

  def test_side_effect_requires
    assert defined?(JSON)
    assert defined?(Pathname)
    assert_respond_to $stdin, :wait_readable
  end
end
