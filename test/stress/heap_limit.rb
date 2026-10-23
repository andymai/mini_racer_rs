# frozen_string_literal: true

# Hits a 64 MiB heap limit over and over, on one context and on contexts made
# and disposed for one hit each, with workloads that end in the script's own
# loop, in Map growth, in JSON.parse and in large arrays. Every hit must raise
# MiniRacerRs::HeapLimitError and leave its context usable, and resident memory
# after the run must stay within the bound of what it was after a warm-up.
#   ruby -Ilib test/stress/heap_limit.rb single|multi [hits] [max_mib]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: heap_limit.rb single|multi [hits] [max_mib]" }
abort "mode must be single or multi" unless %w[single multi].include?(mode)
MiniRacer::Platform.set_flags!(:single_threaded) if mode == "single"
hits = Integer(ARGV.fetch(1, 300))
max_mib = Float(ARGV.fetch(2, 64))
limit = 64 << 20
MiniRacerRs.heap_limit = limit

UNIT = '{"a":12345,"b":"xy","c":[1,2]}'
WORKLOADS = [
  "(() => { const a = []; for (;;) a.push({i: a.length, s: 'k' + a.length, n: [a.length, 1]}); })()",
  "(() => { const m = new Map(); for (let i = 0; ; i++) m.set(i, {i}); })()",
  "(() => { const s = '[' + '#{UNIT},'.repeat(#{limit * 2 / 5 / (UNIT.bytesize + 1)}) + '#{UNIT}]'; " \
  "return JSON.parse(s).length; })()",
  "(() => { const keep = []; for (;;) keep.push(new Array(1 << 20).fill(1.5)); })()",
].freeze

def rss_kib
  if File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
  else
    Integer(`ps -o rss= -p #{Process.pid}`.strip)
  end
end

def settled_rss_kib(context)
  context.low_memory_notification
  3.times { GC.start }
  sleep 0.5
  rss_kib
end

def hit(context, source, index)
  context.eval(source)
  abort "hit #{index}: the script finished"
rescue MiniRacerRs::HeapLimitError => e
  abort "hit #{index}: #{e.message}" unless e.message.end_with?("reached")
  abort "hit #{index}: context unusable" unless context.eval("1 + 1") == 2
end

def round(context, index)
  hit(context, WORKLOADS[index % WORKLOADS.size], index)
  return unless (index % 10).zero?

  fresh = MiniRacer::Context.new
  hit(fresh, WORKLOADS[(index / 10) % WORKLOADS.size], index)
  fresh.dispose
end

context = MiniRacer::Context.new
warmup = 40
warmup.times { |index| round(context, index) }
before = settled_rss_kib(context)
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
hits.times { |index| round(context, warmup + index) }
elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
after = settled_rss_kib(context)
grown_mib = (after - before) / 1024.0
puts format("%<mode>s: %<hits>d hits of a %<limit>d MiB limit in %<s>.1f s; RSS %<b>d KiB -> %<a>d KiB (%<g>+.1f MiB)",
            mode:, hits:, limit: limit >> 20, s: elapsed, b: before, a: after, g: grown_mib,)
context.dispose
abort "RSS grew #{grown_mib.round(1)} MiB, over #{max_mib} MiB" if grown_mib > max_mib
