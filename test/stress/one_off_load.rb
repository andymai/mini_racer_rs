# frozen_string_literal: true

# Loads a different generated script of about 3 MiB into each of several
# contexts in turn, disposing of each after its load, and reads the resident
# memory the process keeps once they are all gone. One astral character makes
# V8 keep each source two bytes a unit, 6 MiB of it. A shared copy that only
# one context read goes at the next large eval, so at most the last script's
# copy may stay. Fails over the limit.
#   ruby -Ilib test/stress/one_off_load.rb single|multi [scripts] [max_mib]

require "mini_racer"

mode = ARGV.fetch(0) { abort "usage: one_off_load.rb single|multi [scripts] [max_mib]" }
abort "mode must be single or multi" unless %w[single multi].include?(mode)
MiniRacer::Platform.set_flags!(:single_threaded) if mode == "single"
scripts = Integer(ARGV.fetch(1, 10))
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
  "// \u{1F600}\n#{functions.join("\n")}\nf#{seed}_0(1, 1);"
end

# Kept live, so starting V8 stays out of the reading.
warm = MiniRacer::Context.new
warm.eval("1 + 1")
sources = Array.new(scripts) { |i| source(i + 1) }
before = settled_rss_kib
sources.each do |text|
  context = MiniRacer::Context.new
  context.eval(text)
  context.dispose
end
kept_mib = (settled_rss_kib - before) / 1024.0
script_mib = sources.first.bytesize / 1_048_576.0
puts "#{mode}: #{scripts} contexts each loading a different #{script_mib.round(1)} MiB script once, " \
     "#{kept_mib.round(1)} MiB kept"
warm.dispose
abort "over #{max_mib} MiB kept" if kept_mib > max_mib
