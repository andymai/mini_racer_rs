# frozen_string_literal: true

require "test_helper"
require "mini_racer"

# Blobs a snapshot is loaded from or a context starts from: the ones V8 would
# abort on, and the one blob each snapshot shares with its contexts.
class SnapshotBlobTest < Minitest::Test
  include RubyProcess

  def blob = MiniRacer::Snapshot.new("var a = 1").dump

  # Deviation behavior:invalid-snapshot@ruby: mini_racer 0.22.1 loads any
  # String, then aborts the process with a V8 fatal error, or a segmentation
  # fault for a blob cut short in its last section, once a context or a warmup
  # reads a blob too short for its header, from another V8, or truncated.
  def test_load_refuses_a_blob_v8_would_abort_on
    good = blob
    other_version = good.dup.tap { |b| b.setbyte(16, b.getbyte(16) ^ 1) }
    refused = {
      "garbage" => "invalid snapshot: truncated or corrupt",
      good.byteslice(0, good.bytesize / 2) => "invalid snapshot: truncated or corrupt",
      good.byteslice(0, good.bytesize - 16) => "invalid snapshot: truncated or corrupt",
      other_version => /\Ainvalid snapshot: not made by V8 \d+\.\d+\.\S+\z/,
      Random.new(1).bytes(good.bytesize) => /\Ainvalid snapshot: not made by V8 /,
    }
    refused.each do |bytes, message|
      error = assert_raises(MiniRacer::SnapshotError) { MiniRacer::Snapshot.load(bytes) }
      assert_match message, error.message
      assert_equal Encoding::BINARY, error.message.encoding
    end
  end

  # Deviation behavior:invalid-snapshot@ruby: mini_racer 0.22.1 aborts here,
  # once V8 reads the blob, so each case runs in a Ruby of its own.
  def test_a_context_or_a_warmup_refuses_a_blob_its_dump_was_cut_down_to
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      snapshot = MiniRacer::Snapshot.new("var a = 1")
      MiniRacer::Context.new(snapshot: snapshot).dispose
      snapshot.dump.slice!(-16, 16)
      [-> { MiniRacer::Context.new(snapshot: snapshot) }, -> { snapshot.warmup!("1") }].each do |use|
        use.call
        puts "used"
      rescue MiniRacer::SnapshotError => e
        puts e.message
      end
    RUBY

    assert_predicate status, :success?, err
    assert_equal "invalid snapshot: truncated or corrupt\n" * 2, out
  end

  # Deviation behavior:invalid-snapshot@ruby: mini_racer 0.22.1 aborts the
  # process, as V8 cannot warm up an empty blob once it has made an isolate.
  def test_a_warmup_of_an_empty_snapshot_raises
    out, err, status = run_ruby(<<~RUBY)
      require "mini_racer"
      [MiniRacer::Snapshot.load(""), MiniRacer::Snapshot.allocate].each do |snapshot|
        snapshot.warmup!("1")
        puts "warmed"
      rescue MiniRacer::SnapshotError => e
        puts e.message
      end
    RUBY

    assert_predicate status, :success?, err
    assert_equal "invalid snapshot: empty\n" * 2, out
  end

  # mini_racer's V8 reads a blob only once every option is accepted.
  def test_an_option_error_comes_before_a_refused_blob
    snapshot = MiniRacer::Snapshot.new("var a = 1")
    snapshot.dump.slice!(-16, 16)

    assert_raises(TypeError) { MiniRacer::Context.new(snapshot: snapshot, timeout: "1") }
    assert_raises(TypeError) { MiniRacer::Context.new(timeout: "1", snapshot: snapshot) }
  end

  def test_an_empty_blob_starts_a_context_without_a_snapshot
    context = MiniRacer::Context.new(snapshot: MiniRacer::Snapshot.load(""))

    assert_equal "undefined", context.eval("typeof a")
  end

  # V8 reads each section by the length it records, as mini_racer's does.
  def test_a_blob_with_bytes_after_its_last_section_still_loads
    snapshot = MiniRacer::Snapshot.load("#{blob}\n")

    assert_equal 1, MiniRacer::Context.new(snapshot: snapshot).eval("a")
  end

  # mini_racer copies the blob `dump` hands out at each Context.new, so a
  # context starts from what that String holds then, and contexts made before
  # keep what they started from.
  def test_a_context_starts_from_what_the_dumped_string_holds_now
    first = MiniRacer::Snapshot.new("var a = 1")
    earlier = MiniRacer::Context.new(snapshot: first)
    first.dump.replace(MiniRacer::Snapshot.new("var a = 2").dump)

    assert_equal 2, MiniRacer::Context.new(snapshot: first).eval("a")
    assert_equal 1, earlier.eval("a")
  end

  def test_contexts_from_one_snapshot_outlive_it_and_its_warmups
    contexts = contexts_from_a_snapshot_let_go
    GC.start

    assert_equal([1] * 9, contexts.map { |context| context.eval("f()") })
  end

  def contexts_from_a_snapshot_let_go
    snapshot = MiniRacer::Snapshot.new("function f() { return 1 }")
    contexts = Array.new(4) { MiniRacer::Context.new(snapshot: snapshot) }
    contexts += Array.new(4) { Thread.new { MiniRacer::Context.new(snapshot: snapshot) } }.map(&:value)
    snapshot.warmup!("f()")
    contexts << MiniRacer::Context.new(snapshot: snapshot)
  end
end
