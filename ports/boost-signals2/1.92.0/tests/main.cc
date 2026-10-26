// Exercises Boost.Signals2: connecting slots, the default last-value combiner, group ordering,
// disconnect, and scoped_connection.
#include <boost/signals2.hpp>

struct add {
    int k;
    int operator()(int x) const { return x + k; }
};
struct counter {
    int* calls;
    int operator()(int x) const { ++*calls; return x; }
};

int main() {
    boost::signals2::signal<int(int)> sig;
    if (sig(1)) return 1;

    int calls = 0;
    boost::signals2::connection c1 = sig.connect(1, add{1});
    sig.connect(0, counter{&calls});
    if (sig.num_slots() != 2) return 1;
    boost::optional<int> r = sig(10);
    if (!r || *r != 11 || calls != 1) return 1;

    c1.disconnect();
    if (c1.connected() || *sig(10) != 10 || calls != 2) return 1;

    {
        boost::signals2::scoped_connection sc(sig.connect(add{5}));
        if (*sig(10) != 15) return 1;
    }
    if (sig.num_slots() != 1 || *sig(10) != 10) return 1;
    return 0;
}
