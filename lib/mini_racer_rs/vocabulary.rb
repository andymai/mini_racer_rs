# frozen_string_literal: true

# The classes below reproduce mini_racer's Ruby vocabulary
# (lib/mini_racer.rb, MIT, Copyright (c) 2016-2019 the mini_racer project authors).

require "pathname"
require "json"
require "io/wait"
require_relative "../mini_racer/version"

module MiniRacer
  class Binary
    attr_reader :data

    def initialize(data)
      raise TypeError, "wrong argument type #{data.class} (expected String)" unless data.is_a?(String)

      @data = data
    end
  end

  class Error < ::StandardError
  end

  class ContextDisposedError < Error
  end

  class PlatformAlreadyInitialized < Error
  end

  class EvalError < Error
  end

  class ParseError < EvalError
  end

  class ScriptTerminatedError < EvalError
  end

  class V8OutOfMemoryError < EvalError
  end

  class RuntimeError < EvalError
    def initialize(message)
      message, *@frames = message.split("\n")
      @frames.map! { "JavaScript #{_1.strip}" }
      super(message)
    end

    def backtrace
      frames = super
      @frames + frames unless frames.nil?
    end
  end

  class ScriptError < EvalError
    def initialize(message)
      message, *@frames = message.split("\n")
      @frames.map! { "JavaScript #{_1.strip}" }
      super(message)
    end

    def backtrace
      frames = super || []
      @frames + frames
    end
  end

  class SnapshotError < Error
    def initialize(message)
      message, *@frames = message.split("\n")
      @frames.map! { "JavaScript #{_1.strip}" }
      super(message)
    end

    def backtrace
      frames = super
      @frames + frames unless frames.nil?
    end
  end

  class Context
    def load(filename)
      eval(File.read(filename))
    end

    def write_heap_snapshot(file_or_io)
      f = nil
      implicit = false

      if file_or_io.is_a?(String)
        f = File.open(file_or_io, "w")
        implicit = true
      else
        f = file_or_io
      end

      raise ArgumentError, "file_or_io" unless f.is_a?(File)

      f.write(heap_snapshot)
    ensure
      f.close if implicit
    end
  end
end
