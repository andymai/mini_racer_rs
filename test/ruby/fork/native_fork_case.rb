# frozen_string_literal: true

# A fork that skips Process._fork, here libc's fork() through Fiddle, as C
# extensions may do: the engine cannot tell whether the child will run Ruby,
# so it does not wait for the runners, and the child leaves every inherited
# context to the parent, idle ones included. mini_racer 0.22.1's child uses
# its idle contexts.
require_relative "prelude"

begin
  require "fiddle"
rescue LoadError
  skip_case "fiddle is not installed"
end

MiniRacer::Platform.set_flags!(:single_threaded)
context = MiniRacer::Context.new
context.eval("var answer = 42")
libc_fork = Fiddle::Function.new(Fiddle::Handle::DEFAULT["fork"], [], Fiddle::TYPE_INT, need_gvl: true)
pid = libc_fork.call
if pid.zero?
  refused = raised(MiniRacer::RuntimeError) { context.eval("answer") }&.start_with?("single-threaded runner:")
  exit!(refused ? 0 : 1)
end
raise "fork failed" if pid.negative?

status = reap(pid)
raise "child exited #{status.inspect}" unless status.success?
raise "parent context" unless context.eval("answer") == 42
