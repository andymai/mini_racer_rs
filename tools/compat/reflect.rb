# frozen_string_literal: true

# Prints the public surface of the MiniRacer namespace on this Ruby engine as
# sorted compat ids. --reference reflects the real mini_racer from
# tools/reference; otherwise mini_racer_rs.

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

before = $LOADED_FEATURES.dup
require "mini_racer"
loaded = $LOADED_FEATURES - before

loaded_gem = defined?(MiniRacerRs) ? "mini_racer_rs" : "mini_racer"
expected_gem = options[:reference] ? "mini_racer" : "mini_racer_rs"
abort "reflect: expected #{expected_gem} but #{loaded_gem} loaded" unless loaded_gem == expected_gem

KEYWORD_PARAMETERS = %i[key keyreq].freeze

def keywords(method)
  method.parameters.filter_map { |kind, name| name if KEYWORD_PARAMETERS.include?(kind) }
end

def walk(mod, ids)
  name = mod.name
  ids << if mod.is_a?(Class)
           "class:#{name} < #{mod.superclass.name}"
         else
           "module:#{name}"
         end
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
    if value.is_a?(Module) && value.name == "#{name}::#{const}"
      walk(value, ids)
    else
      ids << "const:#{name}::#{const}"
    end
  end
end

ids = []
walk(MiniRacer, ids)
CompatExtract.required_features.each do |feature|
  suffix = %r{/#{Regexp.escape(feature)}\.(?:rb|#{Regexp.escape(RbConfig::CONFIG["DLEXT"])})\z}
  ids << "feature:#{feature}" if (before + loaded).any? { |path| suffix.match?(path) }
end
ids.concat(CompatExtract.c_keywords) if options[:reference] && RUBY_ENGINE == "ruby"

report = {
  "engine" => RUBY_ENGINE,
  "gem" => options[:reference] ? "mini_racer #{Gem.loaded_specs.fetch("mini_racer").version}" : "mini_racer_rs",
  "ids" => ids.uniq.sort,
  "stubs" => defined?(MiniRacerRs::STUBS) ? MiniRacerRs::STUBS.sort : [],
}
json = "#{JSON.pretty_generate(report)}\n"
if options[:out]
  FileUtils.mkdir_p(File.dirname(options[:out]))
  File.write(options[:out], json)
else
  $stdout.write(json)
end
