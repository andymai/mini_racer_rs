# frozen_string_literal: true

module MiniRacer
  # mini_racer defines this in its C extension, so it exists only on CRuby.
  class InternalError < EvalError
  end
end
