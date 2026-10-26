// Exercises Boost.STLInterfaces: iterator_interface derives a full random-access iterator from
// three members, and view_interface derives size/empty/front/back/operator[] from begin/end.
#include <boost/stl_interfaces/iterator_interface.hpp>
#include <boost/stl_interfaces/view_interface.hpp>
#include <algorithm>
#include <numeric>

struct int_iterator : boost::stl_interfaces::iterator_interface<
#if !BOOST_STL_INTERFACES_USE_DEDUCED_THIS
                          int_iterator,
#endif
                          std::random_access_iterator_tag, int, int> {
    constexpr int_iterator() noexcept : n_(0) {}
    constexpr explicit int_iterator(int n) noexcept : n_(n) {}
    constexpr int operator*() const noexcept { return n_; }
    constexpr int_iterator& operator+=(std::ptrdiff_t i) noexcept {
        n_ += static_cast<int>(i);
        return *this;
    }
    constexpr std::ptrdiff_t operator-(int_iterator other) const noexcept { return n_ - other.n_; }

private:
    int n_;
};

struct int_range : boost::stl_interfaces::view_interface<int_range> {
    int_range(int first, int last) : first_(first), last_(last) {}
    int_iterator begin() const { return first_; }
    int_iterator end() const { return last_; }

private:
    int_iterator first_, last_;
};

int main() {
    int_iterator it(3);
    if (*(it + 2) != 5 || *++it != 4 || it[3] != 7 || (it + 4) - it != 4) return 1;
    if (!(it < it + 1) || it != int_iterator(4)) return 1;

    int_range r(1, 11);
    if (r.size() != 10 || r.empty() || r.front() != 1 || r.back() != 10 || r[4] != 5) return 1;
    if (std::accumulate(r.begin(), r.end(), 0) != 55) return 1;
    if (std::find(r.begin(), r.end(), 7) - r.begin() != 6) return 1;
    return 0;
}
