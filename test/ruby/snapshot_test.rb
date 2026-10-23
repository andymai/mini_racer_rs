# frozen_string_literal: true

require "test_helper"
require "mini_racer"

class SnapshotTest < Minitest::Test
  # A failed script's objects never reach the blob, so an Intl formatter in
  # one cannot stop the error from being raised.
  def test_a_failing_build_raises_whatever_its_script_left_behind
    error = assert_raises(MiniRacer::SnapshotError) do
      MiniRacer::Snapshot.new("var fmt = new Intl.NumberFormat('en'); throw new Error('boom')")
    end
    assert_equal "Uncaught Error: boom", error.message
  end

  # mini_racer reads the message without transcoding it.
  def test_an_error_message_is_binary_when_it_fits_in_latin1
    latin1 = assert_raises(MiniRacer::SnapshotError) { MiniRacer::Snapshot.new("throw 'é'") }
    wide = assert_raises(MiniRacer::SnapshotError) { MiniRacer::Snapshot.new("throw '東'") }

    assert_equal [Encoding::BINARY, "Uncaught \xE9".b], [latin1.message.encoding, latin1.message]
    assert_equal [Encoding::UTF_8, "Uncaught 東"], [wide.message.encoding, wide.message]
  end

  def test_load_keeps_the_receivers_class_and_the_strings
    subclass = Class.new(MiniRacer::Snapshot)
    blob = Class.new(String).new(MiniRacer::Snapshot.new("var a = 1").dump)
    loaded = subclass.load(blob)

    assert_instance_of subclass, loaded
    assert_instance_of blob.class, loaded.dump
    assert_equal 1, MiniRacer::Context.new(snapshot: loaded).eval("a")
  end

  def test_a_bad_snapshot_keyword_names_the_value_as_ruby_does
    error = assert_raises(TypeError) { MiniRacer::Context.new(snapshot: MiniRacer::Context.new) }
    assert_equal "wrong argument type mini_racer/context (expected mini_racer/snapshot)", error.message
    error = assert_raises(TypeError) { MiniRacer::Context.new(snapshot: String) }
    assert_equal "wrong argument type Class (expected mini_racer/snapshot)", error.message
    renamed = Class.new { def self.to_s = "Renamed" }
    error = assert_raises(TypeError) { MiniRacer::Context.new(snapshot: renamed.new) }
    assert_equal "wrong argument type Renamed (expected mini_racer/snapshot)", error.message
  end

  # Deviation behavior:snapshot-filter-context@ruby: mini_racer 0.22.2 builds
  # the filter's safe context from the snapshot too, so these values come
  # back as nil.
  def test_snapshot_code_cannot_change_how_values_are_copied
    snapshot = MiniRacer::Snapshot.new("Object.getOwnPropertyDescriptors = () => ({})")
    context = MiniRacer::Context.new(snapshot: snapshot)

    assert_equal({ "a" => 1 }, context.eval("({a: 1, f() {}})"))
    assert_equal [1], context.eval("[1, function() {}]")
    assert_equal({}, context.eval("Object.getOwnPropertyDescriptors({a: 1})"))
    snapshot.warmup!("1")

    assert_equal({ "a" => 1 }, MiniRacer::Context.new(snapshot: snapshot).eval("({a: 1, f() {}})"))
  end

  # mini_racer lets go of the warmup's context before it writes the blob, so
  # an object V8 cannot write never reaches it.
  def test_a_warmup_leaves_nothing_of_its_own_context_in_the_blob
    snapshot = MiniRacer::Snapshot.new("function f() { return 1 }")
    snapshot.warmup!("var formatter = new Intl.DateTimeFormat('en'); f()")

    assert_equal [1, "undefined"], MiniRacer::Context.new(snapshot: snapshot).eval("[f(), typeof formatter]")
  end
end
