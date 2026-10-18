# frozen_string_literal: true

require_relative "prelude"

# behavior:multi-threaded-fork@ruby. With V8's default multi-threaded
# platform, a parent that has run JavaScript forks, and the child's new context
# awaits an asynchronous WebAssembly compile, which V8 runs only as a worker
# thread task. Threads do not survive a fork, so mini_racer 0.22.1's child
# never runs the task: on Fedora it was still waiting five seconds later, and
# on Ubuntu 24.04 it hung or aborted with signal 6 when the compile posted to
# V8's worker queue.
COMPILE = <<~JS
  WebAssembly.compile(new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]))
    .then((module) => module instanceof WebAssembly.Module)
JS

parent = MiniRacer::Context.new
raise "parent compile" unless parent.eval_await(COMPILE) == true

child do
  fresh = MiniRacer::Context.new
  compiled = fresh.eval_await(COMPILE)
  raise "child compile: #{compiled.inspect}" unless compiled == true
end
raise "parent" unless parent.eval_await(COMPILE) == true
