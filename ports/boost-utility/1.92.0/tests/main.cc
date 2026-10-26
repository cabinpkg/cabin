// Boost.Utility: string_view, value_initialized, compressed_pair, operators, and base_from_member.
#include <boost/utility/string_view.hpp>
#include <boost/utility/value_init.hpp>
#include <boost/utility/base_from_member.hpp>
#include <boost/compressed_pair.hpp>
#include <boost/operators.hpp>
#include <string>

struct Version : boost::totally_ordered<Version> {
    int n;
    explicit Version(int v) : n(v) {}
    bool operator<(const Version& o) const { return n < o.n; }
    bool operator==(const Version& o) const { return n == o.n; }
};

struct Empty {};

struct Holder : private boost::base_from_member<std::string> {
    std::string::size_type len;
    Holder() : boost::base_from_member<std::string>("member"), len(member.size()) {}
};

int main() {
    std::string s("hello world");
    boost::string_view sv(s);
    if (sv.substr(6) != "world" || sv.find('o') != 4 || !sv.starts_with("hello")) return 1;

    boost::value_initialized<int> zero;
    if (boost::get(zero) != 0) return 1;

    boost::compressed_pair<Empty, int> cp(Empty(), 5);
    if (cp.second() != 5 || sizeof(cp) != sizeof(int)) return 1;

    Version a(1), b(2);
    if (!(a < b) || !(b > a) || !(a <= a) || !(b >= a) || a != Version(1) || a == b) return 1;

    return Holder().len == 6 ? 0 : 1;
}
