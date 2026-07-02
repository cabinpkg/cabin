// The catch2 port's amalgamated TU supplies Catch2's default
// main(), so a test target only defines TEST_CASEs.
#include <catch_amalgamated.hpp>

#include "calc.h"

TEST_CASE("triple scales integers") {
    REQUIRE(triple(1) == 3);
    REQUIRE(triple(-2) == -6);
}
