# frozen_string_literal: true

# Forks from a parent on the default multi-threaded V8, then runs the same
# allocation-heavy loop in a new context in the child and, after it exits, in
# the parent, reading each one's resident memory after a warm-up and at the
# end. V8 hands sweeping and unmapping freed pages to worker threads, which
# do not survive a fork, so a child that cannot run them keeps every page it
# frees. On x86_64 Ubuntu 24.04, over 20,000 evals, such a child grew about
# 19 MiB against its parent's 1 to 2 MiB; a child running its own worker
# threads grew within 3 MiB of its parent. Fails when the child grows more
# than the margin past the parent.
#   ruby -Ilib test/stress/fork_child_memory.rb [evals] [margin_mib]

require "mini_racer"

evals = Integer(ARGV.fetch(0, 20_000))
margin_mib = Float(ARGV.fetch(1, 8))
WORK = <<~JS
  function work(n) {
    const rows = [];
    for (let i = 0; i < n; i++) rows.push({ i, label: "row " + i });
    return rows.length;
  }
JS

def rss_kib
  if File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
  else
    Integer(`ps -o rss= -p #{Process.pid}`.strip)
  end
end

def growth_mib(evals)
  context = MiniRacer::Context.new
  context.eval(WORK)
  1_000.times { context.eval("work(20000)") }
  before = rss_kib
  evals.times { |i| raise "eval #{i} returned the wrong value" unless context.eval("work(20000)") == 20_000 }
  (rss_kib - before) / 1024.0
end

MiniRacer::Context.new.eval("1")
reader, writer = IO.pipe
pid = fork do
  reader.close
  writer.write(growth_mib(evals).to_s)
  writer.close
  exit!(0)
rescue Exception => e # rubocop:disable Lint/RescueException
  warn "child: #{e.class}: #{e.message}"
  exit!(1)
end
writer.close
child = reader.read
reader.close
_, status = Process.wait2(pid)
abort "child failed: #{status.inspect}" unless status.success?
child = Float(child)
parent = growth_mib(evals)
puts format("%<evals>d evals: child grew %<child>+.1f MiB, parent %<parent>+.1f MiB", evals:, child:, parent:)
excess = child - parent
abort "the child grew #{format("%.1f", excess)} MiB more than the parent, over #{margin_mib}" if excess > margin_mib
