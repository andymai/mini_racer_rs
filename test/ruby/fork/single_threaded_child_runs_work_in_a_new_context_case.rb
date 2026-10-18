# frozen_string_literal: true

require_relative "prelude"

# Under `--single-threaded` V8's scavenger still hands its jobs to a worker
# thread, so a child that allocates enough to collect garbage needs a worker of
# its own, and nothing the parent's worker was waiting on.
MiniRacer::Platform.set_flags!(:single_threaded)

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
