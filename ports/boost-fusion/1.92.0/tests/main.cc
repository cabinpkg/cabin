// Exercises Boost.Fusion heterogeneous sequences: vector element access,
// algorithms over mixed types, and a struct adapted as a sequence.
#include <boost/fusion/adapted/struct/adapt_struct.hpp>
#include <boost/fusion/include/algorithm.hpp>
#include <boost/fusion/include/at_c.hpp>
#include <boost/fusion/include/size.hpp>
#include <boost/fusion/include/vector.hpp>
#include <string>

struct Employee {
    std::string name;
    int age;
};
BOOST_FUSION_ADAPT_STRUCT(Employee, name, age)

struct AddSize {
    std::size_t* total;
    void operator()(const std::string& s) const { *total += s.size(); }
    void operator()(int i) const { *total += static_cast<std::size_t>(i); }
};

int main() {
    namespace fusion = boost::fusion;
    fusion::vector<int, std::string, int> v(1, "abc", 10);
    if (fusion::size(v) != 3) return 1;
    if (fusion::at_c<0>(v) != 1 || fusion::at_c<1>(v) != "abc") return 1;

    std::size_t total = 0;
    AddSize f = {&total};
    fusion::for_each(v, f);
    if (total != 14) return 1;

    Employee e = {"Ada", 36};
    fusion::at_c<1>(e) += 1;
    if (e.age != 37 || fusion::at_c<0>(e) != "Ada") return 1;
    total = 0;
    fusion::for_each(e, f);
    if (total != 40) return 1;
    return 0;
}
