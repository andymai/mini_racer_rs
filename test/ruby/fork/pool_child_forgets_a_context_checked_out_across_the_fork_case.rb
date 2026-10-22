# frozen_string_literal: true

# A fork from inside a checkout leaves the child holding its parent's context:
# the child's checkin forgets it, undisposed, and the child's next checkout
# gets a context of its own. The parent checks its context in as usual.
require_relative "prelude"

MiniRacer::Platform.set_flags!(:single_threaded)

built = []
pool = MiniRacerRs::Pool.new(size: 1) do |context|
  built << context
  context.eval("var answer = 42")
end
outer = built.first
pid = pool.with { fork }
if pid.nil?
  Thread.new do
    sleep 5
    warn "child timed out"
    exit! 99
  end
  begin
    inner = pool.with { _1 }
    raise "the child reused its parent's context" if inner.equal?(outer)
    raise "answer" unless pool.with { _1.eval("answer") } == 42
    raise "the inherited context was disposed" unless outer.eval("answer") == 42
    raise "the child built #{built.size - 1} contexts" unless built.size == 2

    exit! 0
  rescue Exception => e # rubocop:disable Lint/RescueException
    warn "child: #{e.class}: #{e.message}"
    exit! 1
  end
end
status = reap(pid)
raise "child failed: #{status.inspect}" unless status.success?
raise "the parent's context changed" unless pool.with { _1 }.equal?(outer)
raise "the parent built more" unless built.size == 1
