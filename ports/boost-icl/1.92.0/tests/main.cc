// Exercises Boost.ICL: an interval_set joining overlapping and touching
// intervals, set difference, an interval_map aggregating overlapping
// segments with +=, and a split_interval_map keeping its borders.
#include <boost/icl/interval_map.hpp>
#include <boost/icl/interval_set.hpp>
#include <boost/icl/split_interval_map.hpp>
#include <utility>

namespace icl = boost::icl;

int main() {
    icl::interval_set<int> busy;
    busy += icl::interval<int>::right_open(1, 5);
    busy += icl::interval<int>::right_open(3, 8);
    busy += icl::interval<int>::right_open(8, 10);  // touches [1,8): joined
    busy += icl::interval<int>::right_open(20, 25);
    if (icl::interval_count(busy) != 2 || icl::cardinality(busy) != 14) return 1;
    if (!icl::contains(busy, 9) || icl::contains(busy, 10) || !icl::contains(busy, 24)) return 1;

    busy -= icl::interval<int>::right_open(4, 6);
    if (icl::interval_count(busy) != 3 || icl::contains(busy, 5) || !icl::contains(busy, 6)) return 1;

    icl::interval_map<int, int> load;
    load += std::make_pair(icl::interval<int>::right_open(0, 10), 1);
    load += std::make_pair(icl::interval<int>::right_open(5, 15), 2);
    if (icl::interval_count(load) != 3) return 1;
    if (load(2) != 1 || load(7) != 3 || load(12) != 2 || load.find(15) != load.end()) return 1;

    // Adding back the difference makes [0,15) uniform: neighbors merge.
    load += std::make_pair(icl::interval<int>::right_open(0, 5), 2);
    load += std::make_pair(icl::interval<int>::right_open(10, 15), 1);
    if (icl::interval_count(load) != 1 || load(0) != 3 || load(14) != 3) return 1;

    icl::split_interval_map<int, int> split;
    split += std::make_pair(icl::interval<int>::right_open(0, 5), 1);
    split += std::make_pair(icl::interval<int>::right_open(5, 10), 1);
    return icl::interval_count(split) == 2 && split(7) == 1 ? 0 : 1;
}
