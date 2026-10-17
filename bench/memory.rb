# frozen_string_literal: true

# One memory scenario, as JSON on stdout for bench/run.rb: resident memory
# from the kernel, read after three collections and a pause so contexts the
# collector released have finished tearing down on their own threads. run.rb
# starts a fresh process per scenario, so every one starts from the same heap.

require "json"
require_relative "external"

role = ARGV.fetch(0)
name = ARGV.fetch(1)
quick = ARGV.include?("--quick")
require "mini_racer"
loaded = defined?(MiniRacerRs) ? "candidate" : "reference"
abort "bench: started as #{role} but the #{loaded} gem loaded" unless role == loaded

ROOT = File.expand_path("..", __dir__)
def bundle(name) = File.read(File.join(ROOT, "tmp", "consumers", "bundles", "#{name}.js"))

def rss_bytes
  kb = if File.exist?("/proc/self/status")
         File.read("/proc/self/status")[/^VmRSS:\s+(\d+)/, 1]
       else
         `ps -o rss= -p #{Process.pid}`.strip
       end
  Integer(kb) * 1024
end

def settled_rss_bytes
  3.times { GC.start }
  sleep 0.5
  rss_bytes
end

# The growth over the resident memory before the contexts, per context, with
# all of them live when it is read.
def per_live_context(count, &)
  before = settled_rss_bytes
  contexts = Array.new(count, &)
  per_context = (settled_rss_bytes - before) / count.to_f
  contexts.each { |context| context.dispose if context.respond_to?(:dispose) }
  { "bytes" => per_context, "contexts" => count }
end

def loaded_context(source) = MiniRacer::Context.new.tap { |context| context.eval(source) }

def churn(count) = count.times { MiniRacer::Context.new.tap { |context| context.eval("1 + 1") }.dispose }

churned = quick ? 2_000 : 20_000

result = begin
  case name
  when "one_context"
    context = MiniRacer::Context.new
    context.eval("1 + 1")
    { "bytes" => settled_rss_bytes.to_f, "contexts" => 1 }
  when "idle_contexts"
    per_live_context(100) { loaded_context("1 + 1") }
  when "babel_contexts"
    babel = bundle("babel")
    per_live_context(20) { loaded_context(babel) }
  when "mui_contexts"
    mui = bundle("mui")
    per_live_context(20) { loaded_context(mui) }
  when "dispose_churn"
    churn(churned / 10)
    before = settled_rss_bytes
    churn(churned)
    { "bytes" => (settled_rss_bytes - before) / churned.to_f, "contexts" => churned }
  when "gc_churn"
    peak = 0
    churned.times do |i|
      MiniRacer::Context.new.eval("1 + 1")
      peak = [peak, rss_bytes].max if (i % 100).zero?
    end
    { "bytes" => peak.to_f, "contexts" => churned }
  when "external_ssr"
    require "execjs"
    abort "bench: ExecJS picked #{ExecJS.runtime.name}" unless ExecJS.runtime.is_a?(ExecJS::MiniRacerRuntime)
    ssr = BenchExternal.source
    per_live_context(quick ? 2 : 5) { ExecJS.compile(ssr) }
  else
    abort "bench: no memory scenario named #{name}"
  end
rescue StandardError, NotImplementedError => e
  { "unsupported" => "#{e.class}: #{e.message[0, 120]}" }
end

gem = defined?(MiniRacerRs) ? "mini_racer_rs #{MiniRacerRs::VERSION}" : "mini_racer #{MiniRacer::VERSION}"
puts JSON.generate({ "gem" => gem, "scenario" => name, "result" => result })
