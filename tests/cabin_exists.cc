#include "helpers.hpp"

#include <boost/ut.hpp>
#include <unistd.h>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin binary exists"_test = [] {
    const auto bin = tests::cabinBinary();
    expect(tests::fs::exists(bin)) << "expected cabin binary";
    expect(::access(bin.c_str(), X_OK) == 0) << "binary should be executable";
  };
}
