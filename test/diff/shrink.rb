# frozen_string_literal: true

# Greedy delta debugging: keep the first smaller variant of a case that still
# diverges until none does. Every variant has fewer nodes, so it terminates.
module DiffShrink
  CONTAINERS = %w[array hash object map set].freeze

  module_function

  def minimize(kase, &)
    current = kase
    loop do
      smaller = variants(current).find(&)
      return current unless smaller

      current = smaller
    end
  end

  def variants(kase)
    case kase["kind"]
    when "js" then item_variants(kase.fetch("tree")).map { |tree| kase.merge("tree" => tree) }
    when "ruby" then list_variants(kase.fetch("args"), min: 1).map { |args| kase.merge("args" => args) }
    else []
    end
  end

  # JS trees and Ruby value trees share the [tag, children] shape for the
  # containers that can shrink; leaves have no smaller variant.
  def item_variants(item)
    tag, children = item
    case tag
    when "array", "set" then children + list_variants(children).map { |list| [tag, list] }
    when "object", "map", "hash" then children.map(&:last) + pair_variants(children).map { |pairs| [tag, pairs] }
    when "shared" then shared_variants(children)
    else []
    end
  end

  # A shared value can shrink and stay shared; dropping the sharing is one more variant.
  def shared_variants(inner)
    still_shared = item_variants(inner).select { |smaller| CONTAINERS.include?(smaller.first) }
    [inner] + still_shared.map { |smaller| ["shared", smaller] }
  end

  def list_variants(list, min: 0)
    removed = list.size > min ? list.each_index.map { |i| list[0...i] + list[(i + 1)..] } : []
    nested = list.each_with_index.flat_map do |item, i|
      item_variants(item).map { |smaller| list[0...i] + [smaller] + list[(i + 1)..] }
    end
    removed + nested
  end

  def pair_variants(pairs)
    removed = pairs.each_index.map { |i| pairs[0...i] + pairs[(i + 1)..] }
    nested = pairs.each_with_index.flat_map do |(key, value), i|
      item_variants(value).map { |smaller| pairs[0...i] + [[key, smaller]] + pairs[(i + 1)..] }
    end
    removed + nested
  end
end
