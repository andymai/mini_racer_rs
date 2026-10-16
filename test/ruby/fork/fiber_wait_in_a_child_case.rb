# frozen_string_literal: true

# A context a fiber has waited on keeps an IO on its runner's descriptor. A
# forked child that takes the context over gives it a runner of its own, so a
# fiber waiting in the child must sleep on that runner's descriptor: on the
# parent's, which the parent's last answer left readable, it would spin. Each
# eval outlasts the spin budget by far, so a real wait costs this thread
# almost no CPU time.
require_relative "prelude"

begin
  require "async"
rescue LoadError
  skip_case "async is not installed"
end

def waited(context)
  started = Process.clock_gettime(Process::CLOCK_THREAD_CPUTIME_ID)
  Sync { raise "answer" unless context.eval("slow()") == 42 }
  Process.clock_gettime(Process::CLOCK_THREAD_CPUTIME_ID) - started
end

MiniRacer::Platform.set_flags!(:single_threaded)
context = MiniRacer::Context.new
context.eval("var answer = 42")
context.eval("globalThis.slow = () => { const end = Date.now() + 200; while (Date.now() < end) {} return answer; }")
waited(context)
child do
  spent = waited(context)
  raise "the child's fiber spun for #{spent} s" if spent > 0.05
end
spent = waited(context)
raise "the parent's fiber spun for #{spent} s" if spent > 0.05
