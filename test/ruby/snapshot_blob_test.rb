# frozen_string_literal: true

require "test_helper"
require "mini_racer"

# Blobs a snapshot is loaded from or a context starts from: the ones refused,
# and the one blob each snapshot shares with its contexts.
class SnapshotBlobTest < Minitest::Test
  def blob = MiniRacer::Snapshot.new("var a = 1").dump

  def word(bytes, at) = bytes.byteslice(at, 4).unpack1("V")

  # What follows the checksum's own word, summed as V8 sums it without zlib.
  def resummed(bytes)
    low = high = 0
    bytes.byteslice(12..).each_byte do |byte|
      low = (low + byte) % 65_535
      high = (high + low) % 65_535
    end
    bytes.dup.tap { |b| b[8, 4] = [(high << 16) | low].pack("V") }
  end

  # mini_racer's checks, in its order and with its messages.
  def test_load_refuses_a_blob_as_mini_racer_does
    good = blob
    refused = {
      "garbage" => "snapshot is too short",
      good.byteslice(0, 88) => "snapshot context table is invalid",
      good.dup.tap { |b| b.setbyte(16, b.getbyte(16) ^ 1) } => "snapshot version mismatch",
      good.dup.tap { |b| b[4, 4] = [2].pack("V") } => "snapshot header is invalid",
      good.dup.tap { |b| b[0, 4] = [0].pack("V") } => "snapshot has no contexts",
      good.dup.tap { |b| b[84, 4] = [1 << 30].pack("V") } => "snapshot shared-heap section is invalid",
      good.byteslice(0, good.bytesize - 16) => "snapshot checksum mismatch",
      "#{good}\n" => "snapshot checksum mismatch",
      good.dup.tap { |b| b.setbyte(b.bytesize - 1, b.getbyte(b.bytesize - 1) ^ 1) } => "snapshot checksum mismatch",
      Random.new(1).bytes(good.bytesize) => "snapshot version mismatch",
    }
    refused.each do |bytes, message|
      error = assert_raises(MiniRacer::SnapshotError) { MiniRacer::Snapshot.load(bytes) }
      assert_equal message, error.message
      assert_equal Encoding::BINARY, error.message.encoding
    end
  end

  # Deviation behavior:invalid-snapshot@ruby: mini_racer 0.22.2 hands V8 a
  # blob whose checksum matches, and V8 aborts the process on a section that
  # does not open with its magic number.
  def test_load_refuses_a_section_v8_aborts_on
    good = blob
    last_context = word(good, 88 + (4 * (word(good, 0) - 1)))
    damaged = resummed(good.dup.tap { |b| b.setbyte(last_context + 3, b.getbyte(last_context + 3) ^ 1) })

    error = assert_raises(MiniRacer::SnapshotError) { MiniRacer::Snapshot.load(damaged) }
    assert_equal "snapshot context section is invalid", error.message
  end

  # Each section starts at an offset in the header, the startup section at
  # the first 8-byte boundary after it, and ends where the next one starts.
  def sections(bytes)
    contexts = word(bytes, 0)
    starts = [(88 + (4 * contexts) + 7) & ~7, word(bytes, 80), word(bytes, 84)]
    starts += Array.new(contexts) { |index| word(bytes, 88 + (4 * index)) }
    starts.zip(starts.drop(1) + [bytes.bytesize])
  end

  def with_payload(bytes, section, length) = resummed(bytes.dup.tap { |b| b[section + 4, 4] = [length].pack("V") })

  # Deviation behavior:invalid-snapshot@ruby: mini_racer 0.22.2 hands V8 a
  # blob whose checksum matches, and V8 reads a section's payload for as long
  # as the section's header says, past the section's end.
  def test_load_refuses_a_section_whose_payload_runs_past_it
    good = blob
    kinds = %w[startup read-only shared-heap context]
    sections(good).each_with_index do |(start, finish), index|
      overlong = with_payload(good, start, finish - start - 8 + 1)

      error = assert_raises(MiniRacer::SnapshotError) { MiniRacer::Snapshot.load(overlong) }
      assert_equal "snapshot #{kinds.fetch(index, "context")} section is invalid", error.message
    end
  end

  # V8 reads each section by the length it records, as mini_racer's does, once
  # the checksum matches, up to the section's end.
  def test_a_blob_whose_checksum_matches_loads_with_bytes_after_its_last_section
    padded = resummed("#{blob}#{"\0" * 16}")
    last, finish = sections(padded).last
    filled = with_payload(padded, last, finish - last - 8)

    [padded, filled].each do |bytes|
      assert_equal 1, MiniRacer::Context.new(snapshot: MiniRacer::Snapshot.load(bytes)).eval("a")
    end
  end

  # mini_racer hands out a copy, so changing it changes no context made after,
  # and no warmup.
  def test_dump_hands_out_a_copy
    snapshot = MiniRacer::Snapshot.new("var a = 1")
    dumped = snapshot.dump
    dumped.replace(MiniRacer::Snapshot.new("var a = 2").dump)
    snapshot.dump.slice!(-16, 16)

    refute_same snapshot.dump, snapshot.dump
    assert_equal 1, MiniRacer::Context.new(snapshot: snapshot).eval("a")
    snapshot.warmup!("a")

    assert_equal 1, MiniRacer::Context.new(snapshot: snapshot).eval("a")
    allocated = MiniRacer::Snapshot.allocate

    refute_same allocated.dump, allocated.dump
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

# An empty snapshot, which stands for none.
class EmptySnapshotTest < Minitest::Test
  include RubyProcess

  # mini_racer warms an empty snapshot up from V8's own, and keeps none of the
  # warmup's state.
  def test_a_warmup_of_an_empty_snapshot_starts_from_v8s_own
    [MiniRacer::Snapshot.load(""), MiniRacer::Snapshot.allocate].each do |snapshot|
      snapshot.warmup!("var b = 1; function g() { return 2 } g()")

      assert_operator snapshot.size, :>, 0
      assert_equal %w[undefined undefined], MiniRacer::Context.new(snapshot: snapshot).eval("[typeof b, typeof g]")
    end
  end

  # A warmup of V8's own snapshot starts as Snapshot.new does, never from an
  # empty blob, so it holds first thing in a process, on the single-threaded
  # platform and in a forked child.
  def test_a_warmup_of_an_empty_snapshot_holds_wherever_it_runs
    warm = <<~RUBY
      snapshot = MiniRacer::Snapshot.load("")
      snapshot.warmup!("1")
      p MiniRacer::Context.new(snapshot: snapshot).eval("1 + 1")
    RUBY
    {
      "first" => warm,
      "single" => "MiniRacer::Platform.set_flags!(:single_threaded)\nMiniRacer::Context.new.eval('1')\n#{warm}",
      "forked" => "MiniRacer::Context.new.eval('1')\npid = fork { #{warm.tr("\n", ";")} exit!(0) }\n" \
                  "exit!(Process.wait2(pid).last.exitstatus)",
    }.each do |label, code|
      out, err, status = run_ruby("require 'mini_racer'\n#{code}")

      assert_predicate status, :success?, "#{label}: #{err}"
      assert_equal "2\n", out, label
    end
  end

  def test_an_empty_blob_starts_a_context_without_a_snapshot
    context = MiniRacer::Context.new(snapshot: MiniRacer::Snapshot.load(""))

    assert_equal "undefined", context.eval("typeof a")
  end
end
