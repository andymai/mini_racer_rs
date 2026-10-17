# frozen_string_literal: true

# Loads a different generated script of about 3 MB into each of several
# contexts and reads resident memory per context once all of them are loaded.
# Parsing and compiling a large script frees most of its working memory back
# to malloc, which keeps it for the process unless asked to return it. Fails
# when a context costs more than the limit.
#   ruby -Ilib test/stress/large_load.rb [contexts] [max_mb]

require "mini_racer"

contexts = Integer(ARGV.fetch(0, 5))
max_mb = Float(ARGV.fetch(1, 16))

def rss_kb
  if File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
  else
    Integer(`ps -o rss= -p #{Process.pid}`.strip)
  end
end

def settled_rss_kb
  3.times { GC.start }
  sleep 0.5
  rss_kb
end

def source(seed)
  functions = Array.new(20_000) do |i|
    "function f#{seed}_#{i}(a, b) { const o = { k#{i}: a + #{i}, s: 'v' + b + #{i} }; " \
      "return [o.k#{i}, o.s, a * #{i % 97}].join('-').length + (b ? f#{seed}_#{[i - 1, 0].max}(a, 0) : 0); }"
  end
  "#{functions.join("\n")}\nf#{seed}_0(1, 1);"
end

# The first context pays for starting V8 once per process.
MiniRacer::Context.new.eval(source(0))
sources = Array.new(contexts) { |i| source(i + 1) }
before = settled_rss_kb
loaded = sources.map { |text| MiniRacer::Context.new.tap { |context| context.eval(text) } }
per_context_mb = (settled_rss_kb - before) / 1024.0 / contexts
megabytes = sources.first.bytesize / 1_000_000.0
puts "#{contexts} contexts with a #{megabytes.round(1)} MB script each: #{per_context_mb.round(1)} MB per context"
loaded.each(&:dispose)
abort "over #{max_mb} MB per context" if per_context_mb > max_mb
