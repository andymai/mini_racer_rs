# frozen_string_literal: true

# One real-library workload, timed the way applications run it, as JSON on
# stdout for bench/run.rb: Babel and Terser through ExecJS as Sprockets runs
# them, React through ExecJS as react-rails does, and a Handlebars template
# called on a context. run.rb starts a fresh process per workload, so what one
# workload leaves behind (contexts awaiting collection, a warm heap) cannot
# change another's numbers. The output's digest lets run.rb confirm both gems
# agree.

require "digest"
require "json"

role = ARGV.fetch(0)
name = ARGV.fetch(1)
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

def median_ns(count, warmup, collect: false)
  output = nil
  run = lambda do
    started = now
    output = yield
    elapsed = now - started
    GC.start if collect
    elapsed
  end
  warmup.times { run.call }
  times = Array.new(count) { run.call }.sort
  { "ns" => times[times.size / 2], "digest" => Digest::SHA256.hexdigest(output.to_s) }
end

scale = quick ? 0.1 : 1
count = ->(n) { [(n * scale).ceil, 2].max }

def react_props = { "name" => "world", "items" => Array.new(1000) { |i| "item #{i}" } }

result = begin
  case name
  when "babel_transform"
    # ExecJS deletes console before it loads a source, and Babel touches it as
    # it loads. V8 keeps optimizing Babel for dozens of transforms, so the
    # warmup is long.
    babel = ExecJS.compile("var console = { log() {}, warn() {}, error() {} };\n#{bundle("babel")}\n" \
                           "function transform(code) { " \
                           "return Babel.transform(code, " \
                           "{ presets: [['env', { targets: 'defaults' }], 'react'] }).code }")
    component = source("bench", "fixtures", "component.jsx")
    median_ns(count[160], 50) { babel.call("transform", component) }
  when "terser_minify"
    terser = ExecJS.compile("#{bundle("terser")}\nfunction minify(code) { return Terser.minify_sync(code).code }")
    library = source("test", "consumers", "js", "node_modules", "handlebars", "dist", "handlebars.runtime.js")
    median_ns(count[30], 5) { terser.call("minify", library) }
  when "react_render"
    react = ExecJS.compile(bundle("react"))
    props = react_props
    median_ns(count[400], 50) { react.call("renderApp", props) }
  when "react_first_render"
    # ExecJS contexts have no dispose, so applications release them through
    # Ruby's garbage collector, and the two gems report their native memory
    # to it differently. Collecting after each sample, outside the timing,
    # keeps teardown out of every sample in both.
    react_source = bundle("react")
    props = react_props
    median_ns(count[20], 3, collect: true) { ExecJS.compile(react_source).call("renderApp", props) }
  when "handlebars_render"
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
    median_ns(count[3000], 300) { handlebars.call("render", rows) }
  else
    abort "bench: no workload named #{name}"
  end
rescue StandardError, NotImplementedError => e
  { "unsupported" => "#{e.class}: #{e.message[0, 120]}" }
end

gem = defined?(MiniRacerRs) ? "mini_racer_rs #{MiniRacerRs::VERSION}" : "mini_racer #{MiniRacer::VERSION}"
engine = if defined?(MiniRacerRs)
           "V8 #{MiniRacerRs::V8_VERSION}"
         else
           "libv8-node #{Gem.loaded_specs["libv8-node"]&.version}"
         end
puts JSON.generate({ "gem" => gem, "engine" => engine, "workload" => name, "result" => result })
