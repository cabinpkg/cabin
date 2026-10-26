// Exercises Boost.Asio without the network: handlers posted to an
// io_context run in order, steady_timer and system_timer expire, a
// stackful coroutine from spawn() (Boost.Context) waits on a timer and
// returns a value, and a strand serializes 1000 handlers that two
// threads run concurrently.
#include <boost/asio.hpp>
#include <boost/asio/spawn.hpp>
#include <chrono>
#include <exception>
#include <thread>
#include <vector>

namespace asio = boost::asio;

int main() {
    asio::io_context io;
    std::vector<int> order;
    asio::post(io, [&order] { order.push_back(1); });
    asio::post(io, [&order] { order.push_back(2); });

    int fired = 0;
    asio::steady_timer steady(io, std::chrono::milliseconds(2));
    steady.async_wait([&fired](const boost::system::error_code& ec) { fired += !ec; });
    asio::system_timer wall(io, std::chrono::milliseconds(1));
    wall.async_wait([&fired](const boost::system::error_code& ec) { fired += !ec; });

    int spawned = 0;
    asio::spawn(
        io,
        [](asio::yield_context yield) {
            asio::steady_timer timer(yield.get_executor(), std::chrono::milliseconds(1));
            timer.async_wait(yield);
            return 42;
        },
        [&spawned](std::exception_ptr e, int value) { spawned = e ? -1 : value; });

    io.run();
    if (order != std::vector<int>{1, 2} || fired != 2 || spawned != 42) return 1;

    asio::io_context pool;
    auto strand = asio::make_strand(pool);
    int counter = 0;  // only ever touched on the strand
    for (int i = 0; i < 1000; ++i) asio::post(strand, [&counter] { ++counter; });
    std::thread a([&pool] { pool.run(); });
    std::thread b([&pool] { pool.run(); });
    a.join();
    b.join();
    return counter == 1000 ? 0 : 1;
}
