// Stores free functions, function objects, and member functions in
// boost::function and checks emptiness, invocation, and target access.
#include <boost/function.hpp>
#include <functional>

static int add(int a, int b) { return a + b; }

struct Scale {
    int factor;
    int apply(int x) const { return x * factor; }
};

int main() {
    boost::function<int(int, int)> f;
    if (!f.empty() || f) return 1;
    try {
        f(1, 2);
        return 1;
    } catch (boost::bad_function_call&) {
    }

    f = &add;
    if (f(2, 3) != 5) return 1;
    if (!f.target<int (*)(int, int)>()) return 1;

    f = std::multiplies<int>();
    if (f(4, 5) != 20 || f.target<int (*)(int, int)>()) return 1;

    boost::function<int(const Scale&, int)> m = &Scale::apply;
    Scale s = {3};
    if (m(s, 7) != 21) return 1;

    f.clear();
    if (!f.empty()) return 1;
    return 0;
}
