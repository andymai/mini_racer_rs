# frozen_string_literal: true

source "https://rubygems.org"

gemspec

gem "activesupport", "~> 8.0"
# The native fork cases call libc's fork() through it. Ruby 4.0 ships without it;
# older Rubies keep it as a default gem, so they need no compiler or libffi for it.
gem "fiddle", "~> 1.1", platforms: :mri, install_if: -> { RUBY_VERSION >= "4.0" }
gem "minitest", "~> 5.25"
gem "rake", "~> 13.2"
gem "rake-compiler", "~> 1.2"
group :lint do
  gem "rubocop", "~> 1.81", require: false
  gem "rubocop-minitest", require: false
  gem "rubocop-performance", require: false
  gem "rubocop-rake", require: false
end
gem "tomlrb", "~> 2.0"
# A fiber scheduler for the tests that wait for a context from a fiber; CRuby only.
group :scheduler do
  gem "async", "~> 2.0"
end
gem "tzinfo-data"
