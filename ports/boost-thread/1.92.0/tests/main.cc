// Exercises Boost.Thread's compiled core: a thread_group sharing a mutex
// and a once_flag, interruption, promise/future (a broken promise reports
// future_category), and thread_specific_ptr cleanup on a std::thread,
// which on Windows runs only through tss_pe.cpp's TLS callbacks.
#include <boost/thread/future.hpp>
#include <boost/thread/lock_guard.hpp>
#include <boost/thread/mutex.hpp>
#include <boost/thread/once.hpp>
#include <boost/thread/thread.hpp>
#include <boost/thread/tss.hpp>
#include <thread>

static int cleanups = 0;
static void cleanup(int* p) { cleanups += *p; delete p; }

int main() {
    boost::once_flag flag = BOOST_ONCE_INIT;
    boost::mutex m;
    int once_calls = 0, counted = 0;
    boost::thread_group group;
    for (int i = 0; i < 4; ++i)
        group.create_thread([&] {
            boost::call_once(flag, [&once_calls] { ++once_calls; });
            for (int j = 0; j < 1000; ++j) {
                boost::lock_guard<boost::mutex> lock(m);
                ++counted;
            }
        });
    group.join_all();
    if (once_calls != 1 || counted != 4000) return 1;

    bool interrupted = false;
    boost::thread sleeper([&interrupted] {
        try {
            boost::this_thread::sleep_for(boost::chrono::seconds(30));
        } catch (const boost::thread_interrupted&) {
            interrupted = true;
        }
    });
    sleeper.interrupt();
    sleeper.join();
    if (!interrupted) return 1;

    boost::promise<int> p;
    auto value = p.get_future();
    boost::thread setter([&p] { p.set_value(42); });
    if (value.get() != 42) return 1;
    setter.join();
    auto broken = boost::promise<int>().get_future();
    try {
        broken.get();
        return 1;
    } catch (const boost::future_error& e) {
        if (e.code().category() != boost::future_category()) return 1;
    }

    boost::thread_specific_ptr<int> slot(cleanup);
    std::thread native([&slot] { slot.reset(new int(7)); });
    native.join();
    return cleanups == 7 ? 0 : 1;
}
