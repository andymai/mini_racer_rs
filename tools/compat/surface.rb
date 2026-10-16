# frozen_string_literal: true

require "json"
require "tomlrb"

# compat/surface.json holds the reference surface of every engine: an id found
# on all engines is bare, one found on some carries @<engine>.
module CompatSurface
  ROOT = File.expand_path("../..", __dir__)
  SURFACE = File.join(ROOT, "compat", "surface.json")
  LEDGER = File.join(ROOT, "compat.toml")
  ENGINES = %w[ruby truffleruby].freeze
  KINDS = %w[module class const imethod smethod kwarg feature behavior].freeze
  STUBBABLE = %w[imethod smethod].freeze

  module_function

  def merge(reports)
    missing = ENGINES - reports.keys
    raise ArgumentError, "no reference report for #{missing.join(", ")}" unless missing.empty?

    reports.values.flatten.uniq.flat_map do |id|
      engines = ENGINES.select { |engine| reports.fetch(engine).include?(id) }
      engines.size == ENGINES.size ? [id] : engines.map { |engine| "#{id}@#{engine}" }
    end.sort
  end

  def split(id)
    base, engine = id.split("@", 2)
    [base, engine]
  end

  def applies?(id, engine)
    on = split(id).last
    on.nil? || on == engine
  end

  def kind(id)
    id.split(":", 2).first
  end

  def load_surface
    JSON.parse(File.read(SURFACE))
  end

  def load_ledger
    Tomlrb.load_file(LEDGER)
  end
end
