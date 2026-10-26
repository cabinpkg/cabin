// Exercises Boost.Interprocess: a uniquely named managed shared memory
// segment holding a named vector, found again through a second mapping
// (offset_ptr keeps it valid at another address), then destroyed and
// removed.
#include <boost/container/vector.hpp>
#include <boost/interprocess/allocators/allocator.hpp>
#include <boost/interprocess/managed_shared_memory.hpp>

#include <cstddef>
#include <cstdio>
#include <ctime>
#include <sstream>
#include <string>
#include <utility>

namespace bip = boost::interprocess;

typedef bip::allocator<int, bip::managed_shared_memory::segment_manager> IntAllocator;
typedef boost::container::vector<int, IntAllocator> IntVector;

static bool run(const char* name) {
    bip::managed_shared_memory segment(bip::create_only, name, 65536);
    IntVector* v = segment.construct<IntVector>("numbers")(IntAllocator(segment.get_segment_manager()));
    for (int i = 0; i < 100; ++i) v->push_back(i);

    bip::managed_shared_memory view(bip::open_only, name);
    std::pair<IntVector*, std::size_t> found = view.find<IntVector>("numbers");
    if (!found.first || found.first->size() != 100 || (*found.first)[42] != 42) return false;
    if (view.find<IntVector>("missing").first) return false;
    return segment.destroy<IntVector>("numbers") && !view.find<IntVector>("numbers").first;
}

int main() {
    // Short (macOS caps POSIX shm names at 31 bytes) and unique per run.
    std::ostringstream os;
    os << "cabin_" << std::hex
       << (static_cast<unsigned long>(std::time(0)) ^
           static_cast<unsigned long>(reinterpret_cast<std::size_t>(&os)));
    const std::string name = os.str();
    bip::shared_memory_object::remove(name.c_str());
    bool ok;
    try {
        ok = run(name.c_str());
    } catch (const bip::interprocess_exception& e) {
        std::fprintf(stderr, "interprocess: %s\n", e.what());
        ok = false;
    }
    return bip::shared_memory_object::remove(name.c_str()) && ok ? 0 : 1;
}
