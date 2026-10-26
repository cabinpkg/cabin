// Exercises the compiled Boost.Test Unit Test Framework: main() comes from
// the archive, which registers, runs and reports these cases and exits
// non-zero on any unexpected failure.
#define BOOST_TEST_MODULE port_probe
#include <boost/test/unit_test.hpp>
#include <stdexcept>
#include <string>
#include <vector>

BOOST_AUTO_TEST_SUITE(checks)

BOOST_AUTO_TEST_CASE(assertions) {
    std::vector<int> v = {1, 2, 3};
    BOOST_TEST(v.size() == 3u);
    BOOST_TEST(std::string("cabin") != "boost");
    BOOST_CHECK_EQUAL(v[0] + v[1], v[2]);
    BOOST_CHECK_CLOSE(0.1 + 0.2, 0.3, 1e-9);
    BOOST_CHECK_THROW(v.at(3), std::out_of_range);
    BOOST_REQUIRE_NO_THROW(v.at(2));
}

// The framework must record the failure and still count the case as passed.
BOOST_AUTO_TEST_CASE(counted_failure, *boost::unit_test::expected_failures(1)) {
    BOOST_CHECK_EQUAL(1, 2);
}

BOOST_AUTO_TEST_SUITE_END()

struct Fixture {
    int value = 41;
};

BOOST_FIXTURE_TEST_CASE(fixture, Fixture) {
    ++value;
    BOOST_TEST(value == 42);
}
