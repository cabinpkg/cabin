// Exercises Boost.Move's portable move emulation on a movable-but-not-copyable
// type and the ownership transfer of boost::movelib::unique_ptr.
#include <boost/move/core.hpp>
#include <boost/move/unique_ptr.hpp>
#include <boost/move/utility.hpp>

class buffer {
    BOOST_MOVABLE_BUT_NOT_COPYABLE(buffer)
public:
    explicit buffer(int n) : data_(new int[n]), size_(n) {}
    buffer(BOOST_RV_REF(buffer) other) : data_(other.data_), size_(other.size_) {
        other.data_ = 0;
        other.size_ = 0;
    }
    buffer& operator=(BOOST_RV_REF(buffer) other) {
        if (this != &other) {
            delete[] data_;
            data_ = other.data_;
            size_ = other.size_;
            other.data_ = 0;
            other.size_ = 0;
        }
        return *this;
    }
    ~buffer() { delete[] data_; }
    int size() const { return size_; }
    bool empty() const { return data_ == 0; }
private:
    int* data_;
    int size_;
};

int main() {
    buffer a(4);
    buffer b(boost::move(a));
    if (!a.empty() || b.size() != 4) return 1;
    buffer c(1);
    c = boost::move(b);
    if (!b.empty() || c.size() != 4) return 1;

    boost::movelib::unique_ptr<int> p(new int(7));
    boost::movelib::unique_ptr<int> q(boost::move(p));
    if (p || !q || *q != 7) return 1;
    return 0;
}
