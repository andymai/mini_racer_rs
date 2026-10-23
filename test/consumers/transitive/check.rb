# frozen_string_literal: true

# Run with BUNDLE_GEMFILE=test/consumers/transitive/Gemfile. Bundler must have
# resolved rtlcss's mini_racer dependency to the shim, and the shim to
# mini_racer_rs; rtlcss itself must behave as status.toml records.

require "rtlcss"

CSS = ".nav { padding-left: 1px; float: right; margin: 1px 2px 3px 4px }"
# The real mini_racer 0.22.2 returns this for CSS.
FLIPPED = ".nav { padding-right: 1px; float: left; margin: 1px 4px 3px 2px }"

specs = Gem.loaded_specs
problems = []
unless specs["mini_racer"]&.metadata&.fetch("mini_racer_rs_shim", nil) == "true"
  problems << "mini_racer resolved to #{specs["mini_racer"]&.full_gem_path.inspect}, not the mini_racer_rs shim"
end
problems << "mini_racer_rs is not the active MiniRacer" unless specs.key?("mini_racer_rs") && defined?(MiniRacerRs)
abort "transitive: #{problems.join("; ")}" unless problems.empty?

puts "rtlcss #{specs["rtlcss"].version} -> mini_racer #{specs["mini_racer"].version} (shim) -> mini_racer_rs #{MiniRacerRs::VERSION}"

status = File.read(File.join(__dir__, "status.toml"))[/^rtlcss = "([^"]+)"$/, 1]
milestones = File.read(File.expand_path("../../upstream/milestones.toml",
                                        __dir__,)).scan(/^\[milestones\.(\w+)\]$/).flatten
unless status == "pass" || (status&.start_with?("xfail:") && milestones.include?(status.delete_prefix("xfail:")))
  abort "transitive: status.toml's rtlcss status #{status.inspect} is neither pass nor xfail:<declared milestone>"
end

begin
  flipped = Rtlcss.flip_css(CSS)
rescue StandardError, NotImplementedError => e
  failure = "#{e.class}: #{e.message}"
end
works = flipped == FLIPPED
if status == "pass"
  abort "transitive: rtlcss regressed: #{failure || flipped.inspect}" unless works
elsif works
  abort "transitive: rtlcss now works; set its status in status.toml to pass"
else
  puts "rtlcss: #{status} (#{failure || flipped.inspect})"
end
