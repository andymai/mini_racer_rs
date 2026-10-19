# frozen_string_literal: true

# A callback's arguments nested 95 deep, the arguments array included, decoded
# on a fiber or a thread with little more than Ruby's margin for a C function
# left, and then the context evaluates and disposes. A decoder that recurses
# until the stack runs out faults, and Ruby's handler then jumps over every
# Rust frame to raise SystemStackError, the context's lock and the runner's
# wait for the reply among them, which leaves the context busy for good. Near
# its limit mini_racer 0.22.1 mostly hands JavaScript `stack level too deep`,
# but also leaves the context busy, hangs, or crashes with signal 11.
require_relative "prelude"

check_the_dive
context = MiniRacer::Context.new
context.attach("f", ->(value) { value.class.name })
%i[fiber thread].each do |where|
  SHAPES.each do |shape, step|
    source = "(() => { const v = #{nested(step, levels: 94)}; try { return f(v) } catch (e) { return e.message } })()"
    whole = context.eval(source)
    GAPS.each do |gap|
      label = "#{shape}, #{gap} levels short of a #{where}'s limit"
      answer = near_the_limit(where, gap) { outcome { context.eval(source) } }
      check_outcome(answer, label, whole, "stack level too deep")
      answer = outcome { context.eval("1 + 1") }
      next if answer == 2

      raise "#{label}: the context no longer evaluates: #{answer.inspect}"
    end
  end
end
usable(context)
