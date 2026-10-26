// Exercises Boost.SmartPtr: shared_ptr/make_shared ownership counts, weak_ptr expiry,
// custom deleters, enable_shared_from_this, scoped_ptr, and intrusive_ptr.
#include <boost/smart_ptr.hpp>
#include <boost/smart_ptr/intrusive_ref_counter.hpp>

static int deleted = 0;

struct node : boost::enable_shared_from_this<node> {
    int value;
    explicit node(int v) : value(v) {}
};
struct counted : boost::intrusive_ref_counter<counted, boost::thread_unsafe_counter> {
    ~counted() { ++deleted; }
};
static void del_int(int* p) { delete p; ++deleted; }

int main() {
    boost::weak_ptr<node> w;
    {
        boost::shared_ptr<node> a = boost::make_shared<node>(7);
        boost::shared_ptr<node> b = a->shared_from_this();
        w = a;
        if (a.use_count() != 2 || b->value != 7 || w.expired()) return 1;
        if (w.lock() != a) return 1;
    }
    if (!w.expired() || w.lock()) return 1;

    { boost::shared_ptr<int> p(new int(3), del_int); }
    if (deleted != 1) return 1;

    {
        boost::scoped_ptr<int> s(new int(4));
        if (*s != 4) return 1;
    }

    {
        boost::intrusive_ptr<counted> i1(new counted);
        boost::intrusive_ptr<counted> i2 = i1;
        if (i1->use_count() != 2) return 1;
    }
    if (deleted != 2) return 1;
    return 0;
}
