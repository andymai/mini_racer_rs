# frozen_string_literal: true

module MiniRacerRs
  # Only the native extension's heap limit raises it, so it exists only on CRuby.
  class HeapLimitError < MiniRacer::V8OutOfMemoryError
  end
end
