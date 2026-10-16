# frozen_string_literal: true

require "json"

# A seeded server-side rendering props structure of about 1 MB of JSON: the
# kind of payload an app passes to a renderer and gets back.
module BenchPayload
  WORDS = %w[alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon
             phi chi psi omega].freeze
  INTL = ["café", "naïve", "Zürich", "東京", "😀 party", "São Paulo"].freeze

  module_function

  def build(target_bytes = 1_000_000, seed: 42)
    random = Random.new(seed)
    text = lambda { |n|
      Array.new(n) do
        random.rand(10).zero? ? INTL.sample(random: random) : WORDS.sample(random: random)
      end.join(" ")
    }
    items = []
    props = { "page" => { "title" => text.call(6), "locale" => "en-US", "path" => "/products/list",
                          "flags" => { "beta" => true, "variant" => "B" }, },
              "items" => items, }
    100.times { items << item(items.size, random, text) } while JSON.generate(props).bytesize < target_bytes
    props
  end

  def item(id, random, text)
    { "id" => id, "sku" => "SKU-#{random.rand(1_000_000)}", "title" => text.call(random.rand(3..8)),
      "price" => random.rand(1..99_999) / 100.0, "rating" => random.rand(5).zero? ? nil : (random.rand * 5).round(3),
      "inStock" => random.rand(2) == 1, "tags" => Array.new(random.rand(1..4)) { WORDS.sample(random: random) },
      "author" => { "id" => random.rand(10_000), "name" => text.call(2) },
      "description" => text.call(random.rand(15..40)), }
  end
end
