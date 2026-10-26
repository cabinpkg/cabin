// Exercises Boost.Iterator adaptors (counting, transform, filter) and a custom
// iterator built with iterator_facade.
#include <boost/iterator/counting_iterator.hpp>
#include <boost/iterator/filter_iterator.hpp>
#include <boost/iterator/iterator_facade.hpp>
#include <boost/iterator/transform_iterator.hpp>
#include <iterator>
#include <numeric>

struct square {
    int operator()(int x) const { return x * x; }
};
struct is_even {
    bool operator()(int x) const { return x % 2 == 0; }
};

class step_iterator : public boost::iterator_facade<step_iterator, const int, boost::forward_traversal_tag> {
public:
    explicit step_iterator(int v = 0) : value_(v) {}
private:
    friend class boost::iterator_core_access;
    void increment() { value_ += 3; }
    bool equal(const step_iterator& other) const { return value_ == other.value_; }
    const int& dereference() const { return value_; }
    int value_;
};

int main() {
    boost::counting_iterator<int> first(1), last(5);
    if (std::accumulate(first, last, 0) != 10) return 1;

    auto sq_first = boost::make_transform_iterator(first, square());
    auto sq_last = boost::make_transform_iterator(last, square());
    if (std::accumulate(sq_first, sq_last, 0) != 30) return 1;

    int data[] = {1, 2, 3, 4, 6};
    auto ev_first = boost::make_filter_iterator(is_even(), data, data + 5);
    auto ev_last = boost::make_filter_iterator(is_even(), data + 5, data + 5);
    if (std::distance(ev_first, ev_last) != 3 || *ev_first != 2) return 1;

    if (std::accumulate(step_iterator(0), step_iterator(12), 0) != 18) return 1;
    return 0;
}
