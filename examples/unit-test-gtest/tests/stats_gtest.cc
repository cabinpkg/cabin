#include <gtest/gtest.h>

#include <stdexcept>
#include <vector>

#include "stats.hpp"

// A fixture shares one sample series across related tests.
class StatsTest : public ::testing::Test {
  protected:
    std::vector<double> series{8.0, 2.0, 6.0, 4.0};
};

TEST_F(StatsTest, MeanOfKnownSeries) { EXPECT_DOUBLE_EQ(mean(series), 5.0); }

TEST_F(StatsTest, MedianOfEvenSeriesAveragesTheMiddlePair) {
    EXPECT_DOUBLE_EQ(median(series), 5.0);
}

TEST(StatsEdgeCases, MedianOfOddSeriesIsTheMiddleElement) {
    EXPECT_DOUBLE_EQ(median({3.0, 1.0, 2.0}), 2.0);
}

TEST(StatsEdgeCases, EmptySeriesThrows) {
    EXPECT_THROW(mean({}), std::invalid_argument);
    EXPECT_THROW(median({}), std::invalid_argument);
}

// The googletest port intentionally excludes gtest_main.cc, so the
// test target supplies its own entry point.
int main(int argc, char **argv) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
