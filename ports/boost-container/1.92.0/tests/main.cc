// Exercises Boost.Container's compiled parts: the polymorphic memory
// resources (default, monotonic and pool resources) and the dlmalloc-backed
// version-2 allocator from the C allocation library.
#include <boost/container/allocator.hpp>
#include <boost/container/pmr/global_resource.hpp>
#include <boost/container/pmr/monotonic_buffer_resource.hpp>
#include <boost/container/pmr/synchronized_pool_resource.hpp>
#include <boost/container/pmr/unsynchronized_pool_resource.hpp>
#include <boost/container/pmr/vector.hpp>
#include <boost/container/vector.hpp>

namespace bc = boost::container;
typedef bc::pmr::vector_of<int>::type pmr_vector;

int main() {
    if (bc::pmr::get_default_resource() != bc::pmr::new_delete_resource()) return 1;

    char buffer[1024];
    bc::pmr::monotonic_buffer_resource mono(buffer, sizeof buffer, bc::pmr::null_memory_resource());
    {
        pmr_vector v(&mono);
        for (int i = 0; i < 10; ++i) v.push_back(i);
        if (v.size() != 10 || v[9] != 9) return 1;
        if (mono.remaining_storage() >= sizeof buffer) return 1;
    }

    bc::pmr::unsynchronized_pool_resource pool;
    bc::pmr::synchronized_pool_resource shared_pool;
    bc::pmr::memory_resource* previous = bc::pmr::set_default_resource(&pool);
    if (previous != bc::pmr::new_delete_resource()) return 1;
    {
        pmr_vector v;
        v.assign(100, 7);
        pmr_vector w(v.begin(), v.end(), &shared_pool);
        if (v.get_allocator().resource() != &pool || w[99] != 7) return 1;
    }
    bc::pmr::set_default_resource(previous);

    bc::vector<int, bc::allocator<int> > dl;
    for (int i = 0; i < 1000; ++i) dl.push_back(i);
    dl.shrink_to_fit();
    return dl.size() == 1000 && dl[999] == 999 ? 0 : 1;
}
