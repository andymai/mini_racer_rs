# frozen_string_literal: true

require "json"

# The floors mini_racer_rs keeps against the real mini_racer 0.22.2, checked on
# the ratios bench/run.rb writes. Given several results, a row fails only if it
# fails in every one, so a second run confirms a miss on a shared runner.
module BenchFloors
  FLOOR = 0.8
  ROUND_TRIP_FLOOR = 2.0
  ROUND_TRIPS = ["eval round trip p50 (us)", "call round trip p50 (us)", "call with timeout p50 (us)",
                 "call in a timeout scope p50 (us)", "callback round trip p50 (us)",].freeze
  # Tail latency on a shared runner moves with its neighbors, not with the gem.
  TAIL = /\bp99/

  module_function

  def floor_for(name) = ROUND_TRIPS.include?(name) ? ROUND_TRIP_FLOOR : FLOOR

  def rows(result)
    result.fetch("summary").reject { |name, _| name.match?(TAIL) }
          .merge(result.fetch("serde").transform_keys { |name| "serde #{name}" })
  end

  def misses(result)
    rows(result).filter_map do |name, row|
      next if row["note"].to_s.start_with?("under ")

      floor = floor_for(name)
      ratio = row["ratio"]
      next if ratio && ratio >= floor

      [name, ratio ? format("%<ratio>.2fx, floor %<floor>.1fx", ratio:, floor:) : "no ratio (#{row["note"]})"]
    end.to_h
  end

  def confirmed(results)
    per_result = results.map { |result| misses(result) }
    per_result.map(&:keys).reduce(:&).to_h { |name| [name, per_result.map { |found| found.fetch(name) }] }
  end
end

if $PROGRAM_NAME == __FILE__
  abort "usage: ruby bench/floors.rb RESULT.json..." if ARGV.empty?

  results = ARGV.map { |path| JSON.parse(File.read(path)) }
  failed = BenchFloors.confirmed(results)
  if failed.empty?
    puts "No gated row is below its floor."
    exit
  end

  puts "| Row | #{ARGV.map { |path| File.basename(path, ".json") }.join(" | ")} |"
  puts "|---|#{"---|" * ARGV.size}"
  failed.each { |name, found| puts "| #{name} | #{found.join(" | ")} |" }
  exit 1
end
