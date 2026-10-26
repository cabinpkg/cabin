// Exercises Boost.Sort's single-threaded algorithms: spreadsort (integers and strings), pdqsort,
// spinsort, and flat_stable_sort, checking sorted order and stability.
#include <boost/sort/flat_stable_sort/flat_stable_sort.hpp>
#include <boost/sort/pdqsort/pdqsort.hpp>
#include <boost/sort/spinsort/spinsort.hpp>
#include <boost/sort/spreadsort/spreadsort.hpp>
#include <algorithm>
#include <string>
#include <utility>
#include <vector>

struct by_key {
    bool operator()(const std::pair<int, int>& a, const std::pair<int, int>& b) const {
        return a.first < b.first;
    }
};

int main() {
    std::vector<int> base;
    unsigned seed = 12345u;
    for (int i = 0; i < 2000; ++i) {
        seed = seed * 1103515245u + 12345u;
        base.push_back(static_cast<int>(seed >> 8) % 1000 - 500);
    }
    std::vector<int> expected = base;
    std::sort(expected.begin(), expected.end());

    std::vector<int> v = base;
    boost::sort::spreadsort::spreadsort(v.begin(), v.end());
    if (v != expected) return 1;
    v = base;
    boost::sort::pdqsort(v.begin(), v.end());
    if (v != expected) return 1;

    std::vector<std::string> words;
    words.push_back("pear"); words.push_back("apple"); words.push_back("fig"); words.push_back("apricot");
    boost::sort::spreadsort::spreadsort(words.begin(), words.end());
    if (words[0] != "apple" || words[1] != "apricot" || words[3] != "pear") return 1;

    std::vector<std::pair<int, int> > recs;
    for (int i = 0; i < 200; ++i) recs.push_back(std::make_pair(base[i] % 5, i));
    std::vector<std::pair<int, int> > recs2 = recs;
    boost::sort::spinsort(recs.begin(), recs.end(), by_key());
    boost::sort::flat_stable_sort(recs2.begin(), recs2.end(), by_key());
    for (std::size_t i = 1; i < recs.size(); ++i) {
        if (recs[i - 1].first > recs[i].first) return 1;
        if (recs[i - 1].first == recs[i].first && recs[i - 1].second > recs[i].second) return 1;
    }
    if (recs != recs2) return 1;
    return 0;
}
