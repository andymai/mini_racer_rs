# frozen_string_literal: true

require "fileutils"
require "json"
require "optparse"
require_relative "extract"

options = { reference: false }
OptionParser.new do |opts|
  opts.on("--reference", "Reflect the real mini_racer (run with BUNDLE_GEMFILE=tools/reference/Gemfile)") do
    options[:reference] = true
  end
  opts.on("--out PATH") { |path| options[:out] = path }
end.parse!

require "mini_racer"

loaded_gem = defined?(MiniRacerRs) ? "mini_racer_rs" : "mini_racer"
expected_gem = options[:reference] ? "mini_racer" : "mini_racer_rs"
abort "reflect: expected #{expected_gem} but #{loaded_gem} loaded" unless loaded_gem == expected_gem

KEYWORD_PARAMETERS = %i[key keyreq].freeze

# Methods written in C or Rust report only a rest parameter, and upstream's
# TruffleRuby eval takes an options hash, so the keywords upstream's C parses
# are found by calling with each one and seeing whether it is refused.
KEYWORD_PROBES = {
  "MiniRacer::Context#initialize" => ->(options) { MiniRacer::Context.new(**options) },
  "MiniRacer::Context#eval" => ->(options) { MiniRacer::Context.new.eval("1", **options) },
  "MiniRacer::Context#eval_await" => ->(options) { MiniRacer::Context.new.eval_await("1", **options) },
}.freeze
PROBE_VALUES = {
  "ensure_gc_after_idle" => 1000, "marshal_stack_depth" => 10, "max_memory" => 100_000_000, "snapshot" => nil,
  "timeout" => 1000, "verbose_exceptions" => true, "filename" => "probe.js",
}.freeze
REFUSED = /bad keyword|unknown keyword|not supported/i

def keywords(method)
  method.parameters.filter_map { |kind, name| name if KEYWORD_PARAMETERS.include?(kind) }
end

# Any other error means the probe itself could not run (on TruffleRuby without
# GraalJS, Context.new fails), and guessing either way would corrupt the surface.
def accepted?(method, key)
  probe = KEYWORD_PROBES.fetch(method) { abort "reflect: no probe for #{method}" }
  probe.call(key.to_sym => PROBE_VALUES.fetch(key) { abort "reflect: no probe value for #{key}" })
  true
rescue ArgumentError, NoMethodError, NotImplementedError
  false
rescue StandardError => e
  return false if REFUSED.match?(e.message)

  abort "reflect: probing #{method} with #{key}: was not refused but failed: #{e.class}: #{e.message}"
end

def walk(mod, ids)
  name = mod.name
  ids << (mod.is_a?(Class) ? "class:#{name} < #{mod.superclass.name}" : "module:#{name}")
  mod.public_instance_methods(false).each do |meth|
    ids << "imethod:#{name}##{meth}"
    keywords(mod.instance_method(meth)).each { |key| ids << "kwarg:#{name}##{meth}:#{key}" }
  end
  if mod.private_method_defined?(:initialize, false)
    keywords(mod.instance_method(:initialize)).each { |key| ids << "kwarg:#{name}#initialize:#{key}" }
  end
  mod.singleton_methods(false).each do |meth|
    ids << "smethod:#{name}.#{meth}"
    keywords(mod.method(meth)).each { |key| ids << "kwarg:#{name}.#{meth}:#{key}" }
  end
  mod.constants(false).each do |const|
    value = mod.const_get(const, false)
    value.is_a?(Module) && value.name == "#{name}::#{const}" ? walk(value, ids) : ids << "const:#{name}::#{const}"
  end
end

ids = []
walk(MiniRacer, ids)
lib = options[:reference] ? File.join(CompatExtract::UPSTREAM, "lib") : File.expand_path("../../lib", __dir__)
CompatExtract.required_features(lib).each { |feature| ids << "feature:#{feature}" }
CompatExtract.c_keywords.each do |id|
  method, _, key = id.delete_prefix("kwarg:").rpartition(":")
  ids << id if accepted?(method, key)
end

report = {
  "engine" => RUBY_ENGINE,
  "gem" => options[:reference] ? "mini_racer #{Gem.loaded_specs.fetch("mini_racer").version}" : "mini_racer_rs",
  "ids" => ids.uniq.sort,
}
unless options[:reference]
  extra_ids = []
  walk(MiniRacerRs, extra_ids)
  report["extra_ids"] = extra_ids.uniq.sort
end
json = "#{JSON.pretty_generate(report)}\n"
if options[:out]
  FileUtils.mkdir_p(File.dirname(options[:out]))
  File.write(options[:out], json)
else
  $stdout.write(json)
end
