# frozen_string_literal: true

# One side of a differential run. It first reports which gem it loaded, then
# for each case line on stdin runs the steps on a fresh context and writes one
# line per step, so a crash keeps the outcomes of the steps before it.

require "json"

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
  exit 1
end

# Bundles run to megabytes, so a step names the file rather than piping its
# source through every worker.
def source(step)
  return step.fetch("source") unless step.key?("file")

  [File.read(step.fetch("file")), step["append"]].compact.join("\n")
rescue SystemCallError => e
  warn "diff worker: cannot read #{step["file"]}: #{e.message}"
  exit 1
end

def execjs(step)
  begin
    require "execjs"
  rescue LoadError => e
    warn "diff worker: #{e.message}"
    exit 1
  end
  ExecJS.compile(source(step)).call(step.fetch("function"), *arguments(step))
end

# A setup step's completion value is never compared, and ending it in undefined
# keeps the step from also testing how a function or object converts.
def evaluate(context, step)
  context.eval(step["setup"] ? "#{source(step)}\n;undefined" : source(step))
end

# A callback either returns what JavaScript passed it or the step's own values.
def attach(context, step)
  callable = case step.fetch("callable")
             when "echo" then proc { |*args| args }
             when "values"
               values = arguments(step)
               proc { values }
             else raise ArgumentError, "unknown callable #{step["callable"]}"
             end
  context.attach(step.fetch("name"), callable)
end

def run(context, step)
  value = case step.fetch("op")
          when "eval" then evaluate(context, step)
          when "call" then context.call(step.fetch("path"), *arguments(step))
          when "attach" then attach(context, step)
          when "execjs" then execjs(step)
          else raise ArgumentError, "unknown step #{step["op"]}"
          end
  step["setup"] ? ["setup"] : DiffCanonical.encode(value)
# What either gem raises is an outcome: NotImplementedError for an option
# mini_racer_rs does not support yet, SystemStackError or NoMemoryError when a
# conversion overflows. Only the harness itself exits the worker.
rescue StandardError, NotImplementedError, SystemStackError, NoMemoryError => e
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
