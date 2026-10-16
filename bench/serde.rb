# frozen_string_literal: true

# mini_racer's own benchmark/serde suite for one role. The suite reports only
# MiniRacer::VERSION, which both gems share, so the role is checked here.

role = ARGV.shift
require "mini_racer"
loaded = defined?(MiniRacerRs) ? "candidate" : "reference"
abort "bench: started as #{role} but the #{loaded} gem loaded" unless role == loaded

load File.expand_path("../vendor/mini_racer/benchmark/serde/bench.rb", __dir__)
