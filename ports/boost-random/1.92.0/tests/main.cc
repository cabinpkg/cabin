// Exercises Boost.Random: the compiled non-deterministic random_device and
// the deterministic engines and distributions built on top of it.
#include <boost/random/mersenne_twister.hpp>
#include <boost/random/random_device.hpp>
#include <boost/random/uniform_int_distribution.hpp>

int main() {
    boost::random::mt19937 engine(42);
    if (engine() != 1608637542u) return 1;

    boost::random::uniform_int_distribution<int> die(1, 6);
    for (int i = 0; i < 100; ++i) {
        int roll = die(engine);
        if (roll < 1 || roll > 6) return 1;
    }

    boost::random::random_device device;
    if ((device.min)() != 0 || (device.max)() == 0) return 1;
    unsigned first = device();
    bool varied = false;
    for (int i = 0; i < 16 && !varied; ++i) varied = device() != first;
    if (!varied) return 1;

    boost::random::mt19937 seeded(device);
    die(seeded);
    return 0;
}
