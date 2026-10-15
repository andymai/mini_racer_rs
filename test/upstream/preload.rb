# frozen_string_literal: true

# RUBYOPT loads this into every Ruby process the upstream suite starts (its own
# test process, `bundle exec` children, Open3 subprocesses). It puts mini_racer_rs
# first and aborts the process if any file from upstream's lib/ is ever loaded,
# so a passing test always exercised mini_racer_rs.

module UpstreamGuard
  OURS = File.expand_path("../../lib", __dir__)
  UPSTREAM = ENV.fetch("MINI_RACER_RS_UPSTREAM_LIB") { File.expand_path("../../vendor/mini_racer/lib", __dir__) }
  UPSTREAM_EXTENSION = /mini_racer_(?:extension|loader)\.(?:so|bundle)\z/

  module_function

  def upstream?(path)
    path.is_a?(String) && (path.start_with?("#{UPSTREAM}/") || UPSTREAM_EXTENSION.match?(path))
  end

  # A trip in a subprocess may only fail one test, so it also leaves a marker the
  # runner checks after every attempt.
  def trip(path)
    report = ENV.fetch("MINI_RACER_RS_UPSTREAM_REPORT", nil)
    File.write("#{report}.guard", "#{path}\n", mode: "a") if report
    warn "upstream harness: #{path} was loaded; the suite must exercise mini_racer_rs"
    exit!(97)
  end

  def audit(from = 0)
    leaked = $LOADED_FEATURES.drop(from).find { upstream?(_1) }
    trip(leaked) if leaked
  end

  module Watch
    # require_relative resolves against its caller's file, so wrapping it would
    # break it; the TracePoint and the exit audit cover relative loads.
    %i[require load].each do |name|
      define_method(name) do |*args, **kwargs|
        known = $LOADED_FEATURES.size
        result = super(*args, **kwargs)
        UpstreamGuard.trip(args.first) if name == :load && UpstreamGuard.upstream?(File.expand_path(args.first.to_s))
        UpstreamGuard.audit(known)
        result
      end
    end
    private :require, :load
  end
end

$LOAD_PATH.delete(UpstreamGuard::OURS)
$LOAD_PATH.unshift(UpstreamGuard::OURS)
Kernel.prepend(UpstreamGuard::Watch)
Kernel.singleton_class.prepend(UpstreamGuard::Watch)
begin
  TracePoint.new(:script_compiled) do |tp|
    path = tp.instruction_sequence&.path
    UpstreamGuard.trip(path) if UpstreamGuard.upstream?(path)
  end.enable
rescue ArgumentError, NotImplementedError
  nil
end

require "mini_racer"
UpstreamGuard.trip("MiniRacerRs (missing)") unless defined?(MiniRacerRs::VERSION)
at_exit { UpstreamGuard.audit }

if ENV["MINI_RACER_RS_UPSTREAM_REPORT"] && ENV["MINI_RACER_RS_UPSTREAM_PARENT"] == Process.ppid.to_s
  require_relative "reporter"
end
