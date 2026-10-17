# frozen_string_literal: true

require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

def script(result)
  functions = Array.new(25_000) { |i| "function f#{i}(a) { return a + #{i} - #{i}; }" }
  "#{functions.join("\n")}\nf0(#{result});"
end

first = MiniRacer::Context.new
raise "first" unless first.eval(script(1000), filename: "bundle.js") == 1000

# The second sighting makes the cache before its eval returns, so a fork
# straight after finds the runner idle and the child takes the context over.
second = MiniRacer::Context.new
raise "second" unless second.eval(script(1000), filename: "bundle.js") == 1000

child do
  raise "taken over" unless second.eval("f24999(7)") == 7

  fresh = MiniRacer::Context.new
  raise "cached" unless fresh.eval(script(1000), filename: "bundle.js") == 1000
  raise "inner function" unless fresh.eval("f24999(7)") == 7

  # Seen twice, so the second reaches the full key next to the cached one.
  2.times do
    other = MiniRacer::Context.new
    raise "same length" unless other.eval(script(2000), filename: "bundle.js") == 2000
  end
end
raise "parent after" unless MiniRacer::Context.new.eval(script(1000), filename: "bundle.js") == 1000
