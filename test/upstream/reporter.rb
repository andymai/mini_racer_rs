# frozen_string_literal: true

require "json"
require "minitest"

# Writes one JSON line when a test starts and one when it finishes, flushed
# immediately, so the runner can tell which test was in flight if the process
# hangs or dies.
module UpstreamReporter
  ROOT = ENV.fetch("MINI_RACER_RS_UPSTREAM_ROOT")

  class Reporter < Minitest::AbstractReporter
    def initialize(path)
      super()
      @out = File.open(path, "a")
      @out.sync = true
    end

    def prerecord(klass, name)
      emit("start", id(klass, name))
    end

    def record(result)
      status = if result.skipped? then "skip"
               elsif result.error? then "error"
               elsif result.passed? then "pass"
               else "fail"
               end
      failure = result.failure
      emit("finish", id(result.klass, result.name), status: status, time: result.time.round(3),
                                                    message: failure&.message.to_s.lines.first.to_s.strip,)
    end

    private

    def id(klass, name)
      klass = Object.const_get(klass) if klass.is_a?(String)
      file = klass.instance_method(name).source_location&.first
      file = file ? file.delete_prefix("#{ROOT}/") : "unknown"
      "#{file}##{klass.name}##{name}"
    end

    def emit(event, id, **fields)
      @out.puts(JSON.generate({ event: event, id: id, pid: Process.pid,
                                at: Process.clock_gettime(Process::CLOCK_MONOTONIC), }.merge(fields)))
    end
  end
end

module Minitest
  def self.plugin_mini_racer_rs_init(_options)
    reporter << UpstreamReporter::Reporter.new(ENV.fetch("MINI_RACER_RS_UPSTREAM_REPORT"))
  end
end
Minitest.extensions << "mini_racer_rs"

# The runner sends USR2 to a test that overran its timeout before killing it.
Signal.trap("USR2") do
  dump = Thread.list.map { |t| "#{t.inspect}\n  #{(t.backtrace || []).join("\n  ")}" }.join("\n")
  File.write("#{ENV.fetch("MINI_RACER_RS_UPSTREAM_REPORT")}.threads", dump)
end
