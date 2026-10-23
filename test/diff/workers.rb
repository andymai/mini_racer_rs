# frozen_string_literal: true

require "fileutils"
require "json"

# Worker processes for a differential run: one per gem, each fed the same cases.
module DiffWorkers
  ROOT = File.expand_path("../..", __dir__)
  WORK = File.join(ROOT, "tmp", "diff")
  EXECJS = File.join(ROOT, "vendor", "execjs", "lib")

  class HarnessError < StandardError; end

  # A long-lived process for one side. It must announce the gem it loaded before
  # any case runs; a crash or hang is the outcome of the step in flight, and the
  # next case gets a fresh process.
  class Worker
    CASE_LIMIT = 30
    START_LIMIT = 120

    attr_reader :gem

    def initialize(role, label = role)
      @role = role
      @log = File.join(WORK, "#{label}-worker.log")
      FileUtils.mkdir_p(WORK)
      start
    end

    def start
      args = DiffWorkers.ruby(@role, File.join(__dir__, "worker.rb"), @role)
      worker_env = DiffWorkers.env(@role)
      spawn = lambda do
        IO.popen(worker_env, args, "r+", chdir: ROOT, pgroup: true, err: [@log, "a"], external_encoding: Encoding::UTF_8)
      end
      @io = DiffWorkers.unbundled(&spawn)
      line = @io.gets if @io.wait_readable(START_LIMIT)
      ready = line && JSON.parse(line)
      raise HarnessError, "the #{@role} worker did not start; see #{@log}" unless ready&.fetch("ready", nil) == @role

      @gem = ready.fetch("gem")
    end

    def submit(kase)
      @io.puts(JSON.generate(kase))
      @io.flush
    rescue Errno::EPIPE, IOError
      nil
    end

    def collect
      outcomes = []
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + CASE_LIMIT
      loop do
        remaining = deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
        return outcomes << timed_out unless remaining.positive? && @io.wait_readable(remaining)

        line = @io.gets
        return outcomes << died unless line

        message = JSON.parse(line)
        return outcomes if message.key?("done")

        outcomes << message.fetch("outcome")
      end
    end

    # A gem that crashes the process dies by a signal. An exit status means a
    # Ruby exception escaped the worker, which is the harness failing, not
    # either gem.
    def died
      @io.close
      status = Process.last_status
      raise HarnessError, "the #{@role} worker exited with #{status.exitstatus}; see #{@log}" unless status.signaled?

      start
      ["crash", "signal #{status.termsig}"]
    end

    def timed_out
      stop
      start
      ["timeout", CASE_LIMIT]
    end

    def stop
      Process.kill("KILL", -@io.pid)
    rescue Errno::ESRCH
      nil
    ensure
      @io.close unless @io.closed?
    end

    def restart
      stop
      start
    end
  end

  module_function

  # How either gem's process is started, shared with bench/run.rb so both
  # measure the same setup.
  def env(role)
    common = { "TZ" => "UTC", "EXECJS_RUNTIME" => "MiniRacer" }
    if role == "candidate"
      # Read before with_unbundled_env drops every BUNDLE_* variable: CI installs
      # some groups only, and the child must ask Bundler for the same groups.
      return common.merge("BUNDLE_GEMFILE" => File.join(ROOT, "Gemfile"),
                          "BUNDLE_WITHOUT" => ENV.fetch("BUNDLE_WITHOUT", nil),)
    end

    common.merge("BUNDLE_GEMFILE" => File.join(ROOT, "tools/reference/Gemfile"),
                 "BUNDLE_PATH" => File.join(ROOT, "tools/reference/vendor/bundle"),)
  end

  def ruby(role, script, *args)
    ["bundle", "exec", "ruby", *(role == "candidate" ? ["-I#{File.join(ROOT, "lib")}"] : []), "-I#{EXECJS}", script,
     *args,]
  end

  def unbundled(&)
    defined?(Bundler) ? Bundler.with_unbundled_env(&) : yield
  end

  # Every side runs a case at the same time; the outcomes are read in turn. A
  # case marked fresh runs first in new processes, so the code cache and the
  # shared sources start empty for it.
  def run_case(workers, kase)
    workers.each(&:restart) if kase["fresh"]
    workers.each { |worker| worker.submit(kase) }
    workers.map(&:collect)
  end
end
