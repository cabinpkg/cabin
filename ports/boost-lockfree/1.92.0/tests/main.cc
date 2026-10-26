// Exercises Boost.Lockfree's stack, its single-producer ring buffer
// between two threads, and its multi-producer queue.
#include <boost/lockfree/queue.hpp>
#include <boost/lockfree/spsc_queue.hpp>
#include <boost/lockfree/stack.hpp>

#include <thread>

static const int count = 10000;

int main() {
    boost::lockfree::stack<int> stack(16);
    for (int i = 0; i < 10; ++i)
        if (!stack.push(i)) return 1;
    int v = 0;
    for (int i = 9; i >= 0; --i)
        if (!stack.pop(v) || v != i) return 1;
    if (stack.pop(v)) return 1;

    boost::lockfree::spsc_queue<int, boost::lockfree::capacity<64> > ring;
    std::thread producer([&ring] {
        for (int i = 0; i < count; ++i)
            while (!ring.push(i)) {
            }
    });
    bool ordered = true;
    for (int expected = 0; expected < count;)
        if (ring.pop(v)) ordered = (v == expected++) && ordered;
    producer.join();
    if (!ordered) return 1;

    boost::lockfree::queue<int> queue(128);
    auto produce = [&queue] {
        for (int i = 1; i <= count; ++i)
            while (!queue.push(i)) {
            }
    };
    std::thread a(produce), b(produce);
    long long sum = 0;
    for (int popped = 0; popped < 2 * count;)
        if (queue.pop(v)) {
            sum += v;
            ++popped;
        }
    a.join();
    b.join();
    return sum == static_cast<long long>(count) * (count + 1) && queue.empty() ? 0 : 1;
}
