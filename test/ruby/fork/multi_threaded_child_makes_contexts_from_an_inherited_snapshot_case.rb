# frozen_string_literal: true

# A multi-threaded child refuses the contexts it inherits, but a snapshot is
# only its blob, so the child makes new contexts from an inherited one.
require_relative "prelude"

snapshot = MiniRacer::Snapshot.new("var base = 40; function add(x) { return base + x }")
inherited = MiniRacer::Context.new(snapshot: snapshot)
raise "parent" unless inherited.eval("add(1)") == 41

child do
  raise "inherited" unless raised(MiniRacer::RuntimeError) { inherited.eval("add(2)") }

  fresh = Array.new(3) { MiniRacer::Context.new(snapshot: snapshot) }
  raise "fresh" unless fresh.map { |context| context.eval("add(2)") } == [42] * 3

  fresh.each(&:dispose)
end
raise "parent after" unless inherited.eval("add(3)") == 43
