// Exercises Boost.Atomic's compiled lock pool: a type too large to be
// lock-free goes through it for every operation, and wait/notify hand a
// flag from one thread to another.
#include <boost/atomic.hpp>
#include <thread>

struct Big {
    int v[16];
};

int main() {
    Big zero = {}, one = {};
    one.v[15] = 1;

    boost::atomic<Big> big(zero);
    if (big.is_lock_free()) return 1;
    big.store(one);
    if (big.load().v[15] != 1) return 1;
    Big expected = zero;
    if (big.compare_exchange_strong(expected, zero) || expected.v[15] != 1) return 1;
    if (big.exchange(zero).v[15] != 1 || big.load().v[15] != 0) return 1;
    big.wait(one);  // the value differs, so this returns at once
    big.notify_all();

    boost::atomic<int> flag(0);
    std::thread waiter([&flag] {
        while (flag.load() == 0) flag.wait(0);
        flag.store(2);
        flag.notify_one();
    });
    flag.store(1);
    flag.notify_one();
    while (flag.load() != 2) flag.wait(1);
    waiter.join();
    return flag.load() == 2 ? 0 : 1;
}
