# frozen_string_literal: true

require_relative "prelude"

# With V8's default multi-threaded platform, a parent that has run JavaScript
# and gone quiet forks, and the child's new context allocates enough for V8 to
# hand garbage collection work to worker threads. The parent's workers do not
# survive the fork, so the child needs workers of its own, and nothing the
# parent's workers were waiting on.
WORK = <<~JS
  function work(n) {
    const rows = [];
    for (let i = 0; i < n; i++) rows.push({ i, label: "row " + i });
    return rows.length;
  }
JS

parent = MiniRacer::Context.new
parent.eval(WORK)
20.times { parent.eval("work(20000)") }
child do
  fresh = MiniRacer::Context.new
  fresh.eval(WORK)
  50.times { raise "work" unless fresh.eval("work(20000)") == 20_000 }
end
raise "parent" unless parent.eval("work(10)") == 10
