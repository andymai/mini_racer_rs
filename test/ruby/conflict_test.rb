# frozen_string_literal: true

require "test_helper"

class ConflictTest < Minitest::Test
  include RubyProcess

  def require_with_spec(metadata)
    run_ruby(<<~RUBY)
      spec = Gem::Specification.new do |s|
        s.name = "mini_racer"
        s.version = "0.22.1"
        s.metadata = #{metadata.inspect}
      end
      Gem.loaded_specs["mini_racer"] = spec
      begin
        require "mini_racer"
        puts MiniRacer::Context.new.eval("1 + 1")
      rescue LoadError => e
        puts e.message
        exit 3
      end
    RUBY
  end

  def test_refuses_to_load_beside_the_mini_racer_gem
    out, _err, status = require_with_spec({})

    assert_equal 3, status.exitstatus
    assert_match "the mini_racer 0.22.1 gem are both active", out
  end

  def test_loads_beside_the_shim
    out, err, status = require_with_spec({ "mini_racer_rs_shim" => "true" })

    assert_predicate status, :success?, err
    assert_equal "2\n", out
  end

  def test_refuses_when_the_upstream_extension_is_loaded
    out, _err, status = run_ruby(<<~RUBY)
      $LOADED_FEATURES << "/gems/mini_racer/lib/mini_racer_extension.#{RbConfig::CONFIG["DLEXT"]}"
      begin
        require "mini_racer"
      rescue LoadError => e
        puts e.message
        exit 3
      end
    RUBY

    assert_equal 3, status.exitstatus
    assert_match "native extension is already loaded", out
  end
end
