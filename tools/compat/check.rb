# frozen_string_literal: true

require "date"
require_relative "surface"
require_relative "../../test/upstream/ledger"

# A status describes the CRuby implementation. On TruffleRuby mini_racer_rs
# runs upstream's own backend, so its surface must match the reference exactly
# whatever the status says.
module CompatCheck
  STATUS = /\A(?:done|deviation|todo:(?<milestone>[\w-]+))\z/
  KEYS = %w[status reason approved depends_on].freeze

  Inputs = Data.define(:ledger, :surface, :candidate, :milestones, :upstream_ledgers, :reference) do
    def initialize(upstream_ledgers: {}, reference: nil, **)
      super
    end
  end

  module_function

  def problems(inputs)
    ledger = inputs.ledger.fetch("ids", {})
    ids = inputs.surface.fetch("ids")
    engine = inputs.candidate.fetch("engine")
    entries = ledger.select { |_, entry| entry.is_a?(Hash) }
    out = (ids - ledger.keys).map { |id| "MISSING #{id}: no compat.toml entry" }
    out.concat((ledger.keys - entries.keys).map { |id| "MALFORMED #{id}: an entry is a table with a status" })
    entries.each do |id, entry|
      kind = CompatSurface.kind(id)
      out << "STALE #{id}: not in compat/surface.json" if kind != "behavior" && !ids.include?(id)
      out.concat(malformed(id, entry, kind, inputs.milestones, entries))
    end
    out.concat(against_candidate(entries, ids, inputs.candidate, engine))
    out.concat(upstream_deviations(entries, inputs.upstream_ledgers))
    out.concat(drift(ids, inputs.reference, engine)) if inputs.reference
    out
  end

  def malformed(id, entry, kind, milestones, entries)
    out = []
    out << "MALFORMED #{id}: unknown kind #{kind}" unless CompatSurface::KINDS.include?(kind)
    suffix = CompatSurface.split(id).last
    out << "MALFORMED #{id}: unknown engine #{suffix}" if suffix && !CompatSurface::ENGINES.include?(suffix)
    out << "MALFORMED #{id}: unknown keys #{(entry.keys - KEYS).join(", ")}" unless (entry.keys - KEYS).empty?
    match = STATUS.match(entry["status"].to_s)
    return out << "MALFORMED #{id}: status #{entry["status"].inspect}" unless match

    status = entry["status"]
    if match[:milestone] && !milestones.include?(match[:milestone])
      out << "UNKNOWN_MILESTONE #{id}: #{match[:milestone]}"
    end
    out << "MALFORMED #{id}: a behavior id is always a deviation" if kind == "behavior" && status != "deviation"
    if CompatSurface.split(id).last == "truffleruby" && status != "done"
      out << "MALFORMED #{id}: TruffleRuby-only surface comes from upstream's backend and is done"
    end
    out.concat(reasons(id, entry, status == "deviation"))
    out.concat(dependencies(id, entry, entries))
  end

  def reasons(id, entry, deviation)
    out = []
    out << "MALFORMED #{id}: a deviation needs a reason" if deviation && entry["reason"].to_s.strip.empty?
    out << "MALFORMED #{id}: only a deviation has a reason" if !deviation && entry.key?("reason")
    out << "MALFORMED #{id}: only a deviation has an approved date" if !deviation && entry.key?("approved")
    out << "MALFORMED #{id}: approved must be YYYY-MM-DD" if entry.key?("approved") && !date?(entry["approved"])
    out
  end

  def dependencies(id, entry, entries)
    Array(entry["depends_on"]).flat_map do |dependency|
      next ["MALFORMED #{id}: depends on #{dependency}, which has no entry"] unless entries[dependency].is_a?(Hash)
      next [] unless entry["status"] == "done" && entries[dependency]["status"] != "done"

      ["MALFORMED #{id}: done, but #{dependency} is #{entries[dependency]["status"]}"]
    end
  end

  def date?(value)
    value.to_s.match?(/\A\d{4}-\d{2}-\d{2}\z/) && Date.iso8601(value.to_s)
  rescue Date::Error
    false
  end

  # A planned composition such as write_heap_snapshot exists before the method
  # it calls works, so it counts as implemented only once its dependencies are done.
  def usable?(entry, entries)
    Array(entry["depends_on"]).all? { |dependency| entries.dig(dependency, "status") == "done" }
  end

  def against_candidate(entries, ids, candidate, engine)
    present = candidate.fetch("ids")
    on_engine = ids.select { |id| CompatSurface.applies?(id, engine) }
    applicable = on_engine.to_h { |id| [CompatSurface.split(id).first, id] }
    out = (present - applicable.keys).map do |id|
      "EXTRA_SURFACE #{id}: mini_racer 0.22.1 has no such #{engine} surface"
    end
    applicable.each do |base, id|
      entry = entries.fetch(id, {})
      found = present.include?(base)
      if engine == "truffleruby"
        out << "CLAIMED_DONE_BUT_ABSENT #{id}: upstream's backend provides it" unless found
        next
      end
      status = entry["status"].to_s
      out << "CLAIMED_DONE_BUT_ABSENT #{id}" if status == "done" && !found
      out << "DEVIATION_BUT_PRESENT #{id}: the deviation says it is absent" if status == "deviation" && found
      out << "TODO_BUT_IMPLEMENTED #{id}: mark it done" if status.start_with?("todo:") && found && usable?(entry,
                                                                                                           entries,)
    end
    out
  end

  def upstream_deviations(entries, upstream_ledgers)
    upstream_ledgers.flat_map do |name, tests|
      tests.flat_map do |test, entry|
        statuses = [entry["status"], *entry.fetch("overrides", {}).values.map { |override| override["status"] }]
        statuses.compact.uniq.filter_map do |status|
          target = status.delete_prefix("deviation:") if status.start_with?("deviation:")
          next unless target && entries.dig(target, "status") != "deviation"

          "UNKNOWN_DEVIATION #{name}: #{test} cites #{target}, which compat.toml does not record as a deviation"
        end
      end
    end
  end

  def drift(ids, reference, engine)
    expected = reference.fetch("ids")
    recorded = ids.select { |id| CompatSurface.applies?(id, engine) }.map { |id| CompatSurface.split(id).first }
    (expected - recorded).map do |id|
      "SURFACE_DRIFT #{id}: mini_racer 0.22.1 has it on #{engine}; compat/surface.json does not"
    end +
      (recorded - expected).map do |id|
        "SURFACE_DRIFT #{id}: in compat/surface.json but not in mini_racer 0.22.1 on #{engine}"
      end
  end
end

if $PROGRAM_NAME == __FILE__
  candidate_path = ARGV.fetch(0) { abort "usage: check.rb <candidate report> [<reference report>]" }
  candidate = JSON.parse(File.read(candidate_path))
  reference = ARGV[1] && JSON.parse(File.read(ARGV[1]))
  upstream = Dir[File.join(CompatSurface::ROOT, "test/upstream/*.status.toml")].to_h do |path|
    [File.basename(path), UpstreamLedger.load(path)]
  end
  inputs = CompatCheck::Inputs.new(ledger: CompatSurface.load_ledger, surface: CompatSurface.load_surface,
                                   candidate: candidate, milestones: UpstreamLedger.milestones,
                                   upstream_ledgers: upstream, reference: reference,)
  problems = CompatCheck.problems(inputs)
  abort problems.sort.join("\n") unless problems.empty?

  puts "compat.toml matches compat/surface.json and mini_racer_rs on #{candidate.fetch("engine")}"
end
