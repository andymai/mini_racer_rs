# frozen_string_literal: true

require "rb_sys/extensiontask"

ENV["RB_SYS_CARGO_PROFILE"] ||= "dev" unless ENV["CI"]

RbSys::ExtensionTask.new("mini_racer_rs", Gem::Specification.load("mini_racer_rs.gemspec")) do |ext|
  ext.lib_dir = "lib/mini_racer_rs"
end
