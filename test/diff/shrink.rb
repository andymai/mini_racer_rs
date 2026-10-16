# frozen_string_literal: true

# Greedy delta debugging: keep the first smaller variant of a case that still
# diverges until none does. Every variant has fewer nodes, so it terminates.
module DiffShrink
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
    when "js" then tree_variants(kase.fetch("tree")).map { |tree| kase.merge("tree" => tree) }
    when "ruby" then list_variants(kase.fetch("args"), min: 1).map { |args| kase.merge("args" => args) }
    else []
    end
  end

  def tree_variants(node)
    tag, children = node
    case tag
    when "array", "set" then children + list_variants(children).map { |list| [tag, list] }
    when "object", "map" then children.map(&:last) + pair_variants(children).map { |pairs| [tag, pairs] }
    when "shared" then [children]
    else []
    end
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

  # A JS tree node and a canonical Ruby value share the [tag, children] shape
  # for the containers that can shrink.
  def item_variants(item)
    case item.first
    when "array" then item[1] + list_variants(item[1]).map { |list| ["array", list] }
    when "hash" then item[1].map(&:last) + pair_variants(item[1]).map { |pairs| ["hash", pairs] }
    else tree_variants(item)
    end
  end
end
