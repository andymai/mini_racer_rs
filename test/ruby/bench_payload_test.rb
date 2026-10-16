# frozen_string_literal: true

require "test_helper"
require_relative "../../bench/payload"

class BenchPayloadTest < Minitest::Test
  def test_the_payload_is_seeded_and_reaches_its_size
    small = BenchPayload.build(20_000)

    assert_equal small, BenchPayload.build(20_000)
    assert_operator JSON.generate(small).bytesize, :>=, 20_000
    refute_equal small, BenchPayload.build(20_000, seed: 7)
  end
end
