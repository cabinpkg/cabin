// Exercises Boost.Cobalt: cobalt::run drives a task that waits on a
// cobalt::io::steady_timer (boost_cobalt_io), drains a generator, reads
// what a promise writes to a channel, and awaits another task.
#include <boost/cobalt.hpp>
#include <boost/cobalt/io/steady_timer.hpp>
#include <chrono>

namespace cobalt = boost::cobalt;

cobalt::generator<int> squares(int n) {
    for (int i = 1; i < n; ++i) co_yield i * i;
    co_return n * n;
}

cobalt::promise<void> producer(cobalt::channel<int>& chan) {
    for (int i = 1; i <= 3; ++i) co_await chan.write(i);
    chan.close();
}

cobalt::task<int> add(int a, int b) { co_return a + b; }

cobalt::task<int> body() {
    cobalt::io::steady_timer timer{std::chrono::milliseconds(1)};
    co_await timer.wait();
    if (!timer.expired()) co_return -1;

    int sum = 0;
    auto gen = squares(3);  // 1, 4, 9
    while (gen) sum += co_await gen;

    cobalt::channel<int> chan{1};
    auto writer = producer(chan);
    for (int i = 0; i < 3; ++i) sum += co_await chan.read();  // 1 + 2 + 3
    co_await writer;

    co_return sum + co_await add(20, 22);
}

int main() { return cobalt::run(body()) == 14 + 6 + 42 ? 0 : 1; }
