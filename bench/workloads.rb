# frozen_string_literal: true

# Real libraries timed the way applications run them, as JSON on stdout for
# bench/run.rb: Babel and Terser through ExecJS as Sprockets runs them, a React
# render through ExecJS as react-rails does, and a Handlebars template called
# on a context. Each output's digest lets run.rb confirm both gems agree.

require "digest"
require "json"

role = ARGV.fetch(0)
quick = ARGV.include?("--quick")
require "mini_racer"
require "execjs"
loaded = defined?(MiniRacerRs) ? "candidate" : "reference"
abort "bench: started as #{role} but the #{loaded} gem loaded" unless role == loaded
abort "bench: ExecJS picked #{ExecJS.runtime.name}" unless ExecJS.runtime.is_a?(ExecJS::MiniRacerRuntime)

ROOT = File.expand_path("..", __dir__)
def source(*path) = File.read(File.join(ROOT, *path))
def bundle(name) = source("tmp", "consumers", "bundles", "#{name}.js")
def now = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)

def median_ns(count, warmup)
  output = nil
  warmup.times { output = yield }
  times = Array.new(count) do
    started = now
    output = yield
    now - started
  end.sort
  { "ns" => times[times.size / 2], "digest" => Digest::SHA256.hexdigest(output.to_s) }
end

scale = quick ? 0.1 : 1
count = ->(n) { [(n * scale).ceil, 2].max }
results = {}

# ExecJS deletes console before it loads a source, and Babel touches it as it
# loads.
babel = ExecJS.compile("var console = { log() {}, warn() {}, error() {} };\n#{bundle("babel")}\n" \
                       "function transform(code) { " \
                       "return Babel.transform(code, { presets: [['env', { targets: 'defaults' }], 'react'] }).code }")
component = source("bench", "fixtures", "component.jsx")
results["babel_transform"] = median_ns(count[40], 5) { babel.call("transform", component) }

terser = ExecJS.compile("#{bundle("terser")}\nfunction minify(code) { return Terser.minify_sync(code).code }")
library = source("test", "consumers", "js", "node_modules", "handlebars", "dist", "handlebars.runtime.js")
results["terser_minify"] = median_ns(count[15], 3) { terser.call("minify", library) }

react_source = bundle("react")
props = { "name" => "world", "items" => Array.new(1000) { |i| "item #{i}" } }
react = ExecJS.compile(react_source)
results["react_render"] = median_ns(count[400], 50) { react.call("renderApp", props) }
results["react_first_render"] = median_ns(count[20], 3) { ExecJS.compile(react_source).call("renderApp", props) }

handlebars = MiniRacer::Context.new
handlebars.eval(bundle("handlebars"))
handlebars.eval(<<~JS)
  globalThis.render = Handlebars.compile(
    "<table>{{#each rows}}<tr><td>{{name}}</td><td>{{email}}</td><td>{{#if admin}}admin{{/if}}</td></tr>{{/each}}</table>"
  )
JS
rows = {
  "rows" => Array.new(50) { |i| { "name" => "User #{i}", "email" => "user#{i}@example.com", "admin" => i.even? } },
}
results["handlebars_render"] = median_ns(count[3000], 300) { handlebars.call("render", rows) }

gem = defined?(MiniRacerRs) ? "mini_racer_rs #{MiniRacerRs::VERSION}" : "mini_racer #{MiniRacer::VERSION}"
puts JSON.generate({ "gem" => gem, "workloads" => results })
