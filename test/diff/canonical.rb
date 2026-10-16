# frozen_string_literal: true

# A tagged, JSON-safe form of every value MiniRacer can return or accept, so
# two processes' results compare exactly: type, encoding, float bits, time zone
# and which containers are the same object all show.
module DiffCanonical
  module_function

  # Containers and errors are numbered for refs; a String or Time met a second
  # time as the same object is marked again, without a number, so container
  # numbering stays what DiffGenerate writes.
  Seen = Struct.new(:ids, :objects)

  def encode(value, seen = Seen.new({}.compare_by_identity, {}.compare_by_identity))
    case value
    when nil then ["nil"]
    when true, false then ["bool", value]
    when Integer then ["int", value.to_s]
    when Float then ["float", value.nan? ? "nan" : [value].pack("G").unpack1("H*")]
    when String then again(value, seen) { string(value) }
    when Symbol then ["symbol", string(value.to_s)]
    when Time then again(value, seen) { ["time", value.to_i.to_s, value.nsec, value.utc_offset, value.utc?] }
    when Array, Hash then container(value, seen)
    when Exception then exception(value, seen)
    else other(value)
    end
  end

  # Frozen strings are skipped: Hash keys are interned and shared by design.
  def again(value, seen)
    tagged = yield
    return tagged if value.frozen?
    return ["again", tagged] if seen.objects.key?(value)

    seen.objects[value] = true
    tagged
  end

  def failure(error)
    ["raise", error.class.name, string(error.message)]
  end

  def string(value)
    text = [Encoding::UTF_8, Encoding::US_ASCII].include?(value.encoding) && value.valid_encoding?
    ["string", value.encoding.name, text ? "text" : "hex", text ? value : value.b.unpack1("H*")]
  end

  def container(value, seen)
    ids = seen.ids
    return ["ref", ids[value]] if ids.key?(value)

    ids[value] = ids.size
    if value.is_a?(Array)
      ["array", value.map { |item| encode(item, seen) }]
    else
      ["hash", value.map { |key, item| [encode(key, seen), encode(item, seen)] }]
    end
  end

  # A returned JavaScript error, with its JavaScript frames and its cause, which
  # can be the error itself.
  def exception(value, seen)
    ids = seen.ids
    return ["ref", ids[value]] if ids.key?(value)

    ids[value] = ids.size
    frames = Array(value.backtrace).map { |frame| string(frame) }
    ["exception", value.class.name, string(value.message), frames, encode(value.cause, seen)]
  end

  def other(value)
    return ["function"] if defined?(MiniRacer::JavaScriptFunction) && value.is_a?(MiniRacer::JavaScriptFunction)
    return ["binary", value.data.b.unpack1("H*")] if defined?(MiniRacer::Binary) && value.is_a?(MiniRacer::Binary)

    ["other", value.class.name]
  end

  def decode(tagged, refs = [])
    tag, *rest = tagged
    case tag
    when "nil" then nil
    when "bool" then rest.fetch(0)
    when "int" then Integer(rest.fetch(0))
    when "float" then rest.fetch(0) == "nan" ? Float::NAN : [rest.fetch(0)].pack("H*").unpack1("G")
    when "string" then decode_string(*rest)
    when "symbol" then decode(rest.fetch(0)).to_sym
    when "time" then decode_time(*rest)
    when "array" then (refs << []).last.tap { |array| rest.fetch(0).each { |item| array << decode(item, refs) } }
    when "hash" then (refs << {}).last.tap do |hash|
      rest.fetch(0).each do |k, v|
        hash[decode(k, refs)] = decode(v, refs)
      end
    end
    when "ref" then refs.fetch(rest.fetch(0))
    when "binary" then MiniRacer::Binary.new([rest.fetch(0)].pack("H*"))
    else raise ArgumentError, "cannot decode #{tag.inspect}"
    end
  end

  def decode_string(encoding, form, payload)
    (form == "text" ? payload.dup : [payload].pack("H*")).force_encoding(encoding)
  end

  def decode_time(seconds, nsec, offset, utc)
    time = Time.at(Integer(seconds), nsec, :nsec)
    utc ? time.utc : time.localtime(offset)
  end
end
