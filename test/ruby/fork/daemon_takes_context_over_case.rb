# frozen_string_literal: true

require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)
context = MiniRacer::Context.new
context.eval("var answer = 42")
reader, writer = IO.pipe
pid = fork do
  reader.close
  Process.daemon(true, true)
  writer.puts(context.eval("answer"))
  exit! 0
end
writer.close
Process.wait(pid)
raise "daemon" unless reader.wait_readable(5) && reader.gets.to_i == 42
raise "parent" unless context.eval("answer") == 42
