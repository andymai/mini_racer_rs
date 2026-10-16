# frozen_string_literal: true

# A `_fork` hook loaded before this gem runs inside its own: from there the
# forking thread cannot wait on a runner, which waits for the fork.
module Probe
  class << self
    attr_accessor :context, :parent, :child
  end

  def _fork
    before = begin
      Probe.context.eval("1")
    rescue MiniRacer::RuntimeError => e
      e.message
    end
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
raise "parent: #{Probe.parent.inspect}" unless Probe.parent == "context busy: fork in progress"
raise "parent after" unless Probe.context.eval("2") == 2
