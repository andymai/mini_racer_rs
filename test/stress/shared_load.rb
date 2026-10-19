# frozen_string_literal: true

# Loads one generated script of about 3 MiB into several contexts and reads
# resident memory per context once all of them are loaded. One astral
# character makes V8 keep the source two bytes a unit, 6 MiB of it. The first
# context to load the script, kept live outside the reading, makes the one
# copy they all read, itself included. On x86_64 Linux a context measured
# about 9 MiB reading the shared copy and 14 MiB with a copy of its own
# (`MINI_RACER_RS_CODE_CACHE=off`), in both platform modes. Fails over the
# limit.
#   ruby -Ilib test/stress/shared_load.rb single|multi [contexts] [max_mib]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: shared_load.rb single|multi [contexts] [max_mib]" }
abort "mode must be single or multi" unless %w[single multi].include?(mode)
MiniRacer::Platform.set_flags!(:single_threaded) if mode == "single"
contexts = Integer(ARGV.fetch(1, 5))
max_mib = Float(ARGV.fetch(2, 14))

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

functions = Array.new(20_000) do |i|
  "function f#{i}(a, b) { const o = { k#{i}: a + #{i}, s: 'v' + b + #{i} }; " \
    "return [o.k#{i}, o.s, a * #{i % 97}].join('-').length + (b ? f#{[i - 1, 0].max}(a, 0) : 0); }"
end
source = "// \u{1F600}\n#{functions.join("\n")}\nf0(1, 1);"

first = MiniRacer::Context.new
first.eval(source)
before = settled_rss_kib
loaded = Array.new(contexts) { MiniRacer::Context.new.tap { |context| context.eval(source) } }
per_context_mib = (settled_rss_kib - before) / 1024.0 / contexts
script_mib = source.bytesize / 1_048_576.0
puts "#{mode}: #{contexts} contexts sharing a #{script_mib.round(1)} MiB script, " \
     "#{per_context_mib.round(1)} MiB per context"
[first, *loaded].each(&:dispose)
abort "over #{max_mib} MiB per context" if per_context_mib > max_mib
