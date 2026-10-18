# frozen_string_literal: true

# Every context made from a snapshot reads the one blob the snapshot shares.
# A child takes an idle one over, makes more from the snapshot it inherited
# and keeps them working once it lets the snapshot go, as the parent does.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

def contexts_from(snapshot) = Array.new(3) { MiniRacer::Context.new(snapshot: snapshot) }

snapshot = MiniRacer::Snapshot.new("var base = 40; function add(x) { return base + x }")
idle = MiniRacer::Context.new(snapshot: snapshot)
child do
  raise "inherited" unless idle.eval("add(2)") == 42

  fresh = contexts_from(snapshot)
  snapshot = nil
  GC.start
  raise "fresh" unless fresh.map { |context| context.eval("add(1)") } == [41] * 3

  [idle, *fresh].each(&:dispose)
end
later = contexts_from(snapshot)
snapshot = nil
GC.start
raise "parent" unless [idle, *later].map { |context| context.eval("add(3)") } == [43] * 4
