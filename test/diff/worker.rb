# frozen_string_literal: true

# One side of a differential run. It first reports which gem it loaded, then
# for each case line on stdin runs the steps on a fresh context and writes one
# line per step, so a crash keeps the outcomes of the steps before it.

require "json"

HARNESS_ERROR = 3

$stdin.set_encoding(Encoding::UTF_8)
$stdout.set_encoding(Encoding::UTF_8)
$stdout.sync = true

role = ARGV.fetch(0)
require "mini_racer"
loaded = defined?(MiniRacerRs) ? "candidate" : "reference"
abort "diff worker: started as #{role} but the #{loaded} gem loaded" unless role == loaded

require_relative "canonical"
require_relative "generate"

gem = if defined?(MiniRacerRs)
        "mini_racer_rs #{MiniRacerRs::VERSION}"
      else
        "mini_racer #{Gem.loaded_specs.fetch("mini_racer").version}"
      end
puts JSON.generate({ "ready" => role, "gem" => gem })

def arguments(step)
  step.fetch("args").map { |arg| DiffCanonical.decode(arg) }
rescue StandardError => e
  warn "diff worker: cannot decode #{JSON.generate(step["args"])}: #{e.message}"
  exit HARNESS_ERROR
end

def run(context, step)
  value = case step.fetch("op")
          when "eval" then context.eval(step.fetch("source"))
          when "call" then context.call(step.fetch("path"), *arguments(step))
          else raise ArgumentError, "unknown step #{step["op"]}"
          end
  step["setup"] ? ["setup"] : DiffCanonical.encode(value)
# mini_racer_rs raises NotImplementedError, which is not a StandardError, for options it does not support yet.
rescue StandardError, NotImplementedError => e
  DiffCanonical.failure(e)
end

$stdin.each_line do |line|
  kase = JSON.parse(line)
  context = MiniRacer::Context.new
  DiffGenerate.steps(kase).each_with_index do |step, index|
    puts JSON.generate({ "step" => index, "outcome" => run(context, step) })
  end
  begin
    context.dispose
  rescue StandardError, NotImplementedError
    nil
  end
  puts JSON.generate({ "done" => kase.fetch("id") })
end
