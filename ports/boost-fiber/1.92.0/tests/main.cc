// Exercises Boost.Fiber's compiled scheduler on Boost.Context: two fibers
// interleave through yield() under round_robin, a buffered channel feeds
// a consumer fiber until closed, a promise is fulfilled from another
// fiber, and fiber_numa's topology() comes from the per-OS source (real
// on Linux and Windows, the throwing stub elsewhere).
#include <boost/fiber/all.hpp>
#include <boost/fiber/numa/topology.hpp>
#include <vector>

namespace fibers = boost::fibers;

static bool numa_matches_platform() {
#if defined(__linux__) || defined(_WIN32)
    std::vector<fibers::numa::node> topo = fibers::numa::topology();
    return !topo.empty() && !topo.front().logical_cpus.empty();
#else
    try {
        fibers::numa::topology();
    } catch (const fibers::fiber_error&) {
        return true;
    }
    return false;
#endif
}

int main() {
    std::vector<int> order;
    fibers::fiber even([&order] {
        for (int i = 0; i < 3; ++i) { order.push_back(2 * i); boost::this_fiber::yield(); }
    });
    fibers::fiber odd([&order] {
        for (int i = 0; i < 3; ++i) { order.push_back(2 * i + 1); boost::this_fiber::yield(); }
    });
    even.join();
    odd.join();
    if (order != std::vector<int>{0, 1, 2, 3, 4, 5}) return 1;

    fibers::buffered_channel<int> channel(4);
    int sum = 0;
    fibers::fiber producer([&channel] {
        for (int i = 1; i <= 10; ++i) channel.push(i);
        channel.close();
    });
    fibers::fiber consumer([&channel, &sum] {
        for (int v : channel) sum += v;
    });
    producer.join();
    consumer.join();
    if (sum != 55) return 1;

    fibers::promise<int> p;
    fibers::future<int> f = p.get_future();
    fibers::fiber setter([&p] {
        boost::this_fiber::sleep_for(std::chrono::milliseconds(1));
        p.set_value(42);
    });
    if (f.get() != 42) return 1;
    setter.join();

    return numa_matches_platform() ? 0 : 1;
}
