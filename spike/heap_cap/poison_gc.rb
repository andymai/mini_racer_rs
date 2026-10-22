# frozen_string_literal: true

# A poisoned context (heap still over the cap after a hit) asked to collect:
# explicit low_memory_notification, and an idle collection.
require "json"
require "mini_racer_rs"
opts = ENV["CASE_IDLE_GC"] ? { ensure_gc_after_idle: 10 } : {}
context = MiniRacer::Context.new(**opts)
js = "globalThis.keep = []; for (;;) keep.push({i: keep.length, s: 'k' + keep.length, n: [keep.length, 1]});"
begin
  context.eval(js)
rescue MiniRacer::V8OutOfMemoryError => e
  puts JSON.generate(hit: e.class.to_s)
end
5.times do |i|
  context.low_memory_notification unless ENV["CASE_IDLE_GC"]
  sleep 0.05
  puts JSON.generate(after_gc: i, used: context.heap_stats[:used_heap_size],
                     limit: context.heap_stats[:heap_size_limit],)
end
later = begin
  context.eval("1+1")
rescue StandardError => e
  e.class.to_s
end
puts JSON.generate(later: later)
context.dispose
puts JSON.generate(done: true)
