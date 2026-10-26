// Exercises Boost.Histogram: regular, category and integer axes with flow bins, indexed
// iteration, reduction, weighted filling, a profile, and the Clopper-Pearson interval
// that Boost.Math's beta distribution computes.
#include <boost/histogram.hpp>
#include <boost/histogram/utility/clopper_pearson_interval.hpp>
#include <cmath>
#include <string>

int main() {
    using namespace boost::histogram;
    auto h = make_histogram(axis::regular<>(4, 0.0, 2.0),
                            axis::category<std::string>({"a", "b"}));
    h(0.1, "a");
    h(0.6, "b");
    h(0.7, "b");
    h(-1.0, "a");
    h(5.0, "z");
    if (h.rank() != 2 || h.axis(0).size() != 4 || h.axis(1).size() != 2) return 1;
    if (h.at(0, 0) != 1 || h.at(1, 1) != 2 || h.at(-1, 0) != 1 || h.at(4, 2) != 1) return 1;
    if (algorithm::sum(h) != 5) return 1;
    double inner = 0;
    for (auto&& x : indexed(h)) inner += *x;
    if (inner != 3) return 1;

    auto r = algorithm::reduce(h, algorithm::rebin(0, 2));
    if (r.axis(0).size() != 2 || r.at(0, 0) != 1 || r.at(0, 1) != 2) return 1;

    auto w = make_weighted_histogram(axis::integer<>(0, 3));
    w(1, weight(2.5));
    w(1, weight(0.5));
    if (w.at(1).value() != 3 || w.at(1).variance() != 6.5) return 1;

    auto p = make_profile(axis::regular<>(2, 0.0, 1.0));
    p(0.2, sample(1.0));
    p(0.3, sample(3.0));
    if (p.at(0).count() != 2 || p.at(0).value() != 2) return 1;

    utility::clopper_pearson_interval<double> cp(utility::confidence_level{0.95});
    auto ci = cp(3, 7);
    if (std::fabs(ci.first - 0.0667395) > 1e-6 || std::fabs(ci.second - 0.6524529) > 1e-6)
        return 1;
    return 0;
}
