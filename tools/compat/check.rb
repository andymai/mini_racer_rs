# frozen_string_literal: true

require "date"
require_relative "surface"
require_relative "../../test/upstream/ledger"

# Checks compat.toml against the reference surface and against what
# mini_racer_rs exposes on this engine. Returns one tagged line per problem.
# A status describes the CRuby implementation: on TruffleRuby mini_racer_rs
# runs upstream's own backend, so its surface must match the reference exactly.
module CompatCheck
  STATUS = /\A(?:done|deviation|todo:(?<milestone>[\w-]+))\z/
  KEYS = %w[status reason approved].freeze

  module_function

  def problems(ledger:, surface:, candidate:, engine:, milestones:)
    entries = ledger.fetch("ids", {})
    ids = surface.fetch("ids")
    out = (ids - entries.keys).map { |id| "MISSING #{id}: no compat.toml entry" }
    entries.each do |id, entry|
      kind = CompatSurface.kind(id)
      out << "STALE #{id}: not in compat/surface.json" if kind != "behavior" && !ids.include?(id)
      out.concat(malformed(id, entry, kind, milestones))
      if CompatSurface.split(id).last == "truffleruby" && entry["status"] != "done"
        out << "MALFORMED #{id}: TruffleRuby-only surface comes from upstream's backend and is done"
      end
    end
    out.concat(against_candidate(entries, ids, candidate, engine))
  end

  def malformed(id, entry, kind, milestones)
    out = []
    out << "MALFORMED #{id}: unknown kind #{kind}" unless CompatSurface::KINDS.include?(kind)
    out << "MALFORMED #{id}: unknown keys #{(entry.keys - KEYS).join(", ")}" unless (entry.keys - KEYS).empty?
    match = STATUS.match(entry["status"].to_s)
    return out << "MALFORMED #{id}: status #{entry["status"].inspect}" unless match

    milestone = match[:milestone]
    out << "UNKNOWN_MILESTONE #{id}: #{milestone}" if milestone && !milestones.include?(milestone)
    deviation = entry["status"] == "deviation"
    out << "MALFORMED #{id}: a deviation needs a reason" if deviation && entry["reason"].to_s.strip.empty?
    out << "MALFORMED #{id}: only a deviation has a reason" if !deviation && entry.key?("reason")
    out << "MALFORMED #{id}: only a deviation has an approved date" if !deviation && entry.key?("approved")
    out << "MALFORMED #{id}: approved must be YYYY-MM-DD" if entry.key?("approved") && !date?(entry["approved"])
    out
  end

  def date?(value)
    value.to_s.match?(/\A\d{4}-\d{2}-\d{2}\z/) && Date.iso8601(value.to_s)
  rescue Date::Error
    false
  end

  # Keyword arguments of methods defined in native code are invisible to
  # reflection on both sides, so the upstream suite verifies them instead.
  def reflectable?(id, candidate)
    CompatSurface.kind(id) != "kwarg" || candidate.fetch("engine") != "ruby"
  end

  def against_candidate(entries, ids, candidate, engine)
    present = candidate.fetch("ids")
    stubs = candidate.fetch("stubs", [])
    applicable = ids.select { |id| CompatSurface.applies?(id, engine) }
    bare = applicable.to_h { |id| [CompatSurface.split(id).first, id] }
    out = (present - bare.keys).map { |id| "EXTRA_SURFACE #{id}: mini_racer 0.22.1 has no such #{engine} surface" }
    bare.each do |base, id|
      status = entries.dig(id, "status").to_s
      next unless reflectable?(base, candidate)

      real = present.include?(base) && !stubs.include?(base)
      if engine == "truffleruby"
        out << "CLAIMED_DONE_BUT_ABSENT #{id}: upstream's backend provides it" if status != "deviation" && !real
        next
      end
      out << "CLAIMED_DONE_BUT_ABSENT #{id}" if status == "done" && !real
      out << "TODO_BUT_IMPLEMENTED #{id}: mark it done" if status.start_with?("todo:") && real
    end
    out
  end
end

if $PROGRAM_NAME == __FILE__
  candidate_path = ARGV.fetch(0) { abort "usage: check.rb <candidate report from reflect.rb>" }
  candidate = JSON.parse(File.read(candidate_path))
  problems = CompatCheck.problems(ledger: CompatSurface.load_ledger, surface: CompatSurface.load_surface,
                                  candidate: candidate, engine: candidate.fetch("engine"),
                                  milestones: UpstreamLedger.milestones,)
  abort problems.sort.join("\n") unless problems.empty?

  puts "compat.toml matches compat/surface.json and mini_racer_rs on #{candidate.fetch("engine")}"
end
