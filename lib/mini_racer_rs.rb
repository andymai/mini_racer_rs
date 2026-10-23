# frozen_string_literal: true

if (spec = Gem.loaded_specs["mini_racer"]) && spec.metadata["mini_racer_rs_shim"] != "true"
  raise LoadError,
        "mini_racer_rs and the mini_racer #{spec.version} gem are both active; " \
        "remove mini_racer from the bundle, or use the shim gemspec from mini_racer_rs"
end

upstream_ext = %r{/mini_racer_(?:extension|loader)\.#{Regexp.escape(RbConfig::CONFIG["DLEXT"])}\z}o
if $LOADED_FEATURES.any? { upstream_ext.match?(_1) }
  raise LoadError, "the mini_racer gem's native extension is already loaded; mini_racer_rs cannot replace it"
end

require_relative "mini_racer_rs/version"
require_relative "mini_racer_rs/vocabulary"

if RUBY_ENGINE == "truffleruby"
  require_relative "mini_racer/truffleruby"
else
  require_relative "mini_racer_rs/internal_error"
  require_relative "mini_racer_rs/heap_limit_error"
  native = ["#{RUBY_VERSION[/\A\d+\.\d+/]}/mini_racer_rs", "mini_racer_rs"].find do |name|
    File.file?(File.join(__dir__, "mini_racer_rs", "#{name}.#{RbConfig::CONFIG["DLEXT"]}"))
  end
  raise LoadError, "mini_racer_rs native extension not found; run `rake compile`" unless native

  require_relative "mini_racer_rs/#{native}"

  unless MiniRacerRs::NATIVE_VERSION == MiniRacerRs::VERSION
    raise LoadError,
          "mini_racer_rs native extension #{MiniRacerRs::NATIVE_VERSION} does not match " \
          "the gem #{MiniRacerRs::VERSION}; run `rake compile`"
  end
end
