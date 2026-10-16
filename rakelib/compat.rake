# frozen_string_literal: true

namespace :compat do
  root = File.expand_path("..", __dir__)

  desc "Reflect the MiniRacer surface on this engine into tmp/compat (REFERENCE=1: the real mini_racer)"
  task :reflect do
    reference = ENV["REFERENCE"] == "1"
    out = File.join(root, "tmp/compat/#{reference ? "reference" : "candidate"}-#{RUBY_ENGINE}.json")
    mkdir_p File.dirname(out)
    if reference
      env = { "BUNDLE_GEMFILE" => File.join(root, "tools/reference/Gemfile"),
              "BUNDLE_PATH" => File.join(root, "tools/reference/vendor/bundle"), }
      Bundler.with_unbundled_env do
        sh env, "bundle", "exec", "ruby", "tools/compat/reflect.rb", "--reference", "--out", out
      end
    else
      ruby "-I#{File.join(root, "lib")}", "tools/compat/reflect.rb", "--out", out
    end
  end

  desc "Merge tmp/compat/reference-{ruby,truffleruby}.json into compat/surface.json"
  task :surface do
    require_relative "../tools/compat/surface"

    reports = CompatSurface::ENGINES.to_h do |engine|
      path = File.join(root, "tmp/compat/reference-#{engine}.json")
      abort "missing #{path}; run REFERENCE=1 rake compat:reflect on #{engine}" unless File.exist?(path)

      [engine, JSON.parse(File.read(path))]
    end
    gems = reports.values.map { |report| report.fetch("gem") }.uniq
    abort "reference reports disagree on the gem: #{gems.join(", ")}" unless gems.size == 1

    surface = { "mini_racer" => gems.first.split.last,
                "ids" => CompatSurface.merge(reports.transform_values { |report| report.fetch("ids") }), }
    File.write(CompatSurface::SURFACE, "#{JSON.pretty_generate(surface)}\n")
    puts "#{CompatSurface::SURFACE}: #{surface["ids"].size} ids"
  end

  desc "Check compat.toml against compat/surface.json and mini_racer_rs on this engine"
  task check: [*(RUBY_ENGINE == "ruby" ? [:compile] : []), :reflect] do
    ruby "tools/compat/check.rb", File.join(root, "tmp/compat/candidate-#{RUBY_ENGINE}.json")
  end

  desc "Regenerate COMPATIBILITY.md, the README compat region and lib/mini_racer_rs/stubs.rb (CHECK=1: verify only)"
  task :render do
    ruby "tools/compat/render.rb", *(ENV["CHECK"] == "1" ? ["--check"] : [])
  end
end
