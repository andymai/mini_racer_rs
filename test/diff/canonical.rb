# frozen_string_literal: true

# A tagged, JSON-safe form of every value MiniRacer can return or accept, so
# two processes' results compare exactly: type, encoding, float bits and which
# containers are the same object all show.
module DiffCanonical
  module_function

  def encode(value, ids = {}.compare_by_identity)
    case value
    when nil then ["nil"]
    when true, false then ["bool", value]
    when Integer then ["int", value.to_s]
    when Float then ["float", value.nan? ? "nan" : [value].pack("G").unpack1("H*")]
    when String then string(value)
    when Symbol then ["symbol", value.to_s]
    when Time then ["time", value.to_i.to_s, value.nsec]
    when Array, Hash then container(value, ids)
    when Exception then ["exception", value.class.name, value.message]
    else other(value)
    end
  end

  def string(value)
    text = value.encoding == Encoding::UTF_8 && value.valid_encoding?
    ["string", value.encoding.name, text ? "text" : "hex", text ? value : value.b.unpack1("H*")]
  end

  def container(value, ids)
    return ["ref", ids[value]] if ids.key?(value)

    ids[value] = ids.size
    if value.is_a?(Array)
      ["array", value.map { |item| encode(item, ids) }]
    else
      ["hash", value.map { |key, item| [encode(key, ids), encode(item, ids)] }]
    end
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
    when "symbol" then rest.fetch(0).to_sym
    when "time" then Time.at(Integer(rest.fetch(0)), rest.fetch(1), :nsec)
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
end
