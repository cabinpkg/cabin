// Exercises Boost.ScopeExit: BOOST_SCOPE_EXIT runs on block exit with by-reference and
// by-value captures, including the commit/rollback idiom.
#include <boost/scope_exit.hpp>
#include <vector>

static bool push_checked(std::vector<int>& v, int value, bool commit) {
    v.push_back(value);
    BOOST_SCOPE_EXIT(&v, &commit) {
        if (!commit) v.pop_back();
    } BOOST_SCOPE_EXIT_END
    return commit;
}

int main() {
    int n = 0;
    int snapshot = 5;
    {
        BOOST_SCOPE_EXIT(&n, snapshot) {
            n += snapshot;
        } BOOST_SCOPE_EXIT_END
        snapshot = 100;
        if (n != 0) return 1;
    }
    if (n != 5) return 1;

    std::vector<int> v;
    push_checked(v, 1, true);
    push_checked(v, 2, false);
    if (v.size() != 1 || v[0] != 1) return 1;
    return 0;
}
