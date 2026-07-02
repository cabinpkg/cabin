#include <gtest/gtest.h>

#include "calc.h"

TEST(Calc, AddsSmallIntegers) { EXPECT_EQ(add(2, 2), 4); }

TEST(Calc, AddsNegatives) { EXPECT_EQ(add(-2, 2), 0); }

// The googletest port intentionally excludes gtest_main.cc, so the
// test target supplies its own entry point.
int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
