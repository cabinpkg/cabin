// Exercises Boost.Pool: a raw fixed-size pool and an object_pool that constructs and
// destroys objects, checking ownership with is_from.
#include <boost/pool/object_pool.hpp>
#include <boost/pool/pool.hpp>

struct counted {
    static int live;
    int value;
    explicit counted(int v) : value(v) { ++live; }
    ~counted() { --live; }
};
int counted::live = 0;

int main() {
    boost::pool<> raw(sizeof(int));
    int* a = static_cast<int*>(raw.malloc());
    int* b = static_cast<int*>(raw.malloc());
    if (a == 0 || b == 0 || a == b) return 1;
    *a = 1;
    *b = 2;
    if (!raw.is_from(a) || !raw.is_from(b)) return 1;
    raw.free(a);
    raw.free(b);

    {
        boost::object_pool<counted> objs;
        counted* x = objs.construct(7);
        counted* y = objs.construct(8);
        if (x->value + y->value != 15 || counted::live != 2) return 1;
        if (!objs.is_from(x)) return 1;
        objs.destroy(x);
        if (counted::live != 1) return 1;
    }
    // The object_pool destructor destroys the objects still allocated.
    if (counted::live != 0) return 1;
    return 0;
}
