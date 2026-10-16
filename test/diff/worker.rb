# frozen_string_literal: true

# One side of a differential run: reads a case per line on stdin, runs its steps
# on a fresh context, and writes the canonical outcome of each step.

require "json"

role = ARGV.fetch(0)
require "mini_racer"
loaded = defined?(MiniRacerRs) ? "candidate" : "reference"
unless role == loaded
  abort "diff worker: started as #{role} but #{loaded == "candidate" ? "mini_racer_rs" : "mini_racer"} loaded"
end

require_relative "canonical"
require_relative "generate"

$stdout.sync = true
$stdin.each_line do |line|
  kase = JSON.parse(line)
  context = MiniRacer::Context.new
  outcomes = DiffGenerate.steps(kase).map do |step|
    value = case step.fetch("op")
            when "eval" then context.eval(step.fetch("source"))
            when "call" then context.call(step.fetch("path"), *step.fetch("args").map do |arg|
              DiffCanonical.decode(arg)
            end,)
            else raise ArgumentError, "unknown step #{step["op"]}"
            end
    DiffCanonical.encode(value)
  # mini_racer_rs raises NotImplementedError, which is not a StandardError, for options it does not support yet.
  rescue StandardError, NotImplementedError => e
    ["raise", e.class.name, e.message]
  end
  begin
    context.dispose
  rescue StandardError, NotImplementedError
    nil
  end
  puts JSON.generate({ "id" => kase.fetch("id"), "outcomes" => outcomes })
end
