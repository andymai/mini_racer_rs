# frozen_string_literal: true

# A callback that forks: the child returns from it into a dispatch whose
# runner stayed with the parent, so that dispatch, a nested one made before
# returning and every later use of the context raise in the child, which can
# still make new contexts. The parent's dispatch carries on. mini_racer
# 0.22.1's child hangs instead.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
context = MiniRacer::Context.new
parent = Process.pid
pid = nil
nested = nil
spawn = proc do
  pid = fork
  next "parent" if pid

  nested = raised(MiniRacer::RuntimeError) { context.eval("1") }
  "child"
end
context.attach("spawn", spawn)

outer = begin
  context.eval("spawn()")
rescue MiniRacer::RuntimeError => e
  e.message
end

if Process.pid != parent
  later = raised(MiniRacer::RuntimeError) { context.eval("1") }
  refused = [outer, nested, later].all? { |message| message&.start_with?("single-threaded runner:") }
  exit!(refused && MiniRacer::Context.new.eval("6 * 7") == 42 ? 0 : 1)
end

raise "parent got #{outer.inspect}" unless outer == "parent"

status = reap(pid)
raise "child exited #{status.inspect}" unless status.success?
raise "parent context" unless context.eval("1") == 1
