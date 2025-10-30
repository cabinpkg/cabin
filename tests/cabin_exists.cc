#include "helpers.hpp"

#include <boost/ut.hpp>

int main() {
  using boost::ut::expect;
  using boost::ut::operator""_test;

  "cabin binary exists"_test = [] {
    const auto bin = tests::cabinBinary();
    expect(tests::fs::exists(bin)) << "expected cabin binary";
    const auto perms = tests::fs::status(bin).permissions();
    const auto execPerms = tests::fs::perms::owner_exec
                           | tests::fs::perms::group_exec
                           | tests::fs::perms::others_exec;
    expect((perms & execPerms) != tests::fs::perms::none)
        << "binary should be executable";
  };
}
