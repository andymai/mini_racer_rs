# frozen_string_literal: true

# Loads a different generated script of about 3 MiB into each of several
# contexts and reads resident memory per context once all of them are loaded.
# Parsing and compiling a large script frees most of its working memory back
# to malloc, which keeps it for the process unless asked to return it. On
# x86_64 Linux a context measured about 10 MiB with that memory handed back
# and 24 MiB without, in both platform modes. Fails over the limit.
#   ruby -Ilib test/stress/large_load.rb single|multi [contexts] [max_mib]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: large_load.rb single|multi [contexts] [max_mib]" }
abort "mode must be single or multi" unless %w[single multi].include?(mode)
MiniRacer::Platform.set_flags!(:single_threaded) if mode == "single"
contexts = Integer(ARGV.fetch(1, 5))
max_mib = Float(ARGV.fetch(2, 16))

def rss_kib
  if File.exist?("/proc/self/status")
    File.read("/proc/self/status")[/VmRSS:\s+(\d+)/, 1].to_i
  else
    Integer(`ps -o rss= -p #{Process.pid}`.strip)
  end
end

def settled_rss_kib
  3.times { GC.start }
  sleep 0.5
  rss_kib
end

def source(seed)
  functions = Array.new(20_000) do |i|
    "function f#{seed}_#{i}(a, b) { const o = { k#{i}: a + #{i}, s: 'v' + b + #{i} }; " \
      "return [o.k#{i}, o.s, a * #{i % 97}].join('-').length + (b ? f#{seed}_#{[i - 1, 0].max}(a, 0) : 0); }"
  end
  "#{functions.join("\n")}\nf#{seed}_0(1, 1);"
end

# Kept live, so starting V8 and this context's teardown stay out of the reading.
warm = MiniRacer::Context.new
warm.eval(source(0))
sources = Array.new(contexts) { |i| source(i + 1) }
before = settled_rss_kib
loaded = sources.map { |text| MiniRacer::Context.new.tap { |context| context.eval(text) } }
per_context_mib = (settled_rss_kib - before) / 1024.0 / contexts
script_mib = sources.first.bytesize / 1_048_576.0
puts "#{mode}: #{contexts} contexts with a #{script_mib.round(1)} MiB script each, " \
     "#{per_context_mib.round(1)} MiB per context"
[warm, *loaded].each(&:dispose)
abort "over #{max_mib} MiB per context" if per_context_mib > max_mib
