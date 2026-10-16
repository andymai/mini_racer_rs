# frozen_string_literal: true

# One thread's fork pauses in a `_fork` hook with the GVL released while the
# main thread forks. The main thread's child counts only its own fork from
# Ruby, which has returned, so a later libc fork in it is not one whose child
# runs Ruby: that grandchild refuses the context the child took over.

module SlowFork
  def _fork
    if Thread.current[:slow]
      Thread.current[:entered] = true
      sleep 0.5
    end
    super
  end
end
# Prepended before the gem loads, so it runs inside the gem's own hook.
Process.singleton_class.prepend(SlowFork)

require_relative "prelude"

begin
  require "fiddle"
rescue LoadError
  skip_case "fiddle is not installed"
end

MiniRacer::Platform.set_flags!(:single_threaded)
idle = MiniRacer::Context.new
idle.eval("var answer = 42")
libc_fork = Fiddle::Function.new(Fiddle::Handle::DEFAULT["fork"], [], Fiddle::TYPE_INT, need_gvl: true)

slow = Thread.new do
  Thread.current[:slow] = true
  child { nil }
end
sleep 0.01 until slow[:entered]

child do
  raise "idle" unless idle.eval("answer") == 42

  pid = libc_fork.call
  if pid.zero?
    refused = raised(MiniRacer::RuntimeError) { idle.eval("answer") }&.start_with?("single-threaded runner:")
    exit!(refused ? 0 : 1)
  end
  raise "fork failed" if pid.negative?

  status = reap(pid)
  raise "grandchild exited #{status.inspect}" unless status.success?
end
slow.join
raise "parent" unless idle.eval("answer") == 42
