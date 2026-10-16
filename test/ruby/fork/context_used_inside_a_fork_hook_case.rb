# frozen_string_literal: true

# A `_fork` hook loaded before this gem runs inside its own, before the fork
# itself: there contexts work as anywhere else, on the forking thread and on
# others.
module Probe
  class << self
    attr_accessor :context, :parent, :child
  end

  def _fork
    before = [Probe.context.eval("1"), Thread.new { Probe.context.eval("2") }.value]
    MiniRacer::Context.new.dispose
    pid = super
    if pid.zero?
      Probe.child = Probe.context.eval("1")
    else
      Probe.parent = before
    end
    pid
  end
end
Process.singleton_class.prepend(Probe)

require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
Probe.context = MiniRacer::Context.new
Probe.context.eval("1")
child { raise "child after the hook: #{Probe.child.inspect}" unless Probe.child == 1 }
raise "parent: #{Probe.parent.inspect}" unless Probe.parent == [1, 2]
raise "parent after" unless Probe.context.eval("2") == 2
