// Exercises Boost.Accumulators: one accumulator_set computes count, sum,
// min, max, mean, variance and moments over a sample stream, with
// dependency resolution between the features; a rolling window keeps
// only the newest samples; a weighted mean honors sample weights; and
// the P-square median estimate lands between the two middle samples.
#include <boost/accumulators/accumulators.hpp>
#include <boost/accumulators/statistics/count.hpp>
#include <boost/accumulators/statistics/max.hpp>
#include <boost/accumulators/statistics/mean.hpp>
#include <boost/accumulators/statistics/median.hpp>
#include <boost/accumulators/statistics/min.hpp>
#include <boost/accumulators/statistics/moment.hpp>
#include <boost/accumulators/statistics/rolling_mean.hpp>
#include <boost/accumulators/statistics/stats.hpp>
#include <boost/accumulators/statistics/sum.hpp>
#include <boost/accumulators/statistics/variance.hpp>
#include <boost/accumulators/statistics/weighted_mean.hpp>
#include <cmath>

namespace acc = boost::accumulators;

int main() {
    acc::accumulator_set<double, acc::stats<acc::tag::count, acc::tag::sum, acc::tag::min, acc::tag::max,
                                            acc::tag::variance, acc::tag::moment<2>, acc::tag::median> >
        all;
    const double samples[] = {2, 4, 4, 4, 5, 5, 7, 9};
    for (double s : samples) all(s);
    if (acc::count(all) != 8 || acc::sum(all) != 40 || acc::min(all) != 2 || acc::max(all) != 9) return 1;
    // The variance is updated incrementally, so it carries rounding error.
    if (acc::mean(all) != 5 || std::abs(acc::variance(all) - 4) > 1e-12 || acc::moment<2>(all) != 29) return 2;
    if (acc::median(all) < 4 || acc::median(all) > 5) return 3;

    acc::accumulator_set<int, acc::stats<acc::tag::rolling_mean> > rolling(acc::tag::rolling_window::window_size = 3);
    for (int i = 1; i <= 10; ++i) rolling(i);
    if (acc::rolling_mean(rolling) != 9) return 4;

    acc::accumulator_set<double, acc::stats<acc::tag::weighted_mean>, double> weighted;
    weighted(1.0, acc::weight = 3.0);
    weighted(5.0, acc::weight = 1.0);
    return acc::weighted_mean(weighted) == 2.0 ? 0 : 5;
}
