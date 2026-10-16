# frozen_string_literal: true

# A child of a quiesced fork forks again through libc before it first uses
# what it inherited. That second fork does not wait for runners, and must not
# undo the first: the child still takes its idle context over.
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
child do
  pid = libc_fork.call
  exit!(0) if pid.zero?
  raise "fork failed" if pid.negative?

  status = reap(pid)
  raise "grandchild exited #{status.inspect}" unless status.success?
  raise "idle" unless idle.eval("answer") == 42
end
raise "parent" unless idle.eval("answer") == 42
