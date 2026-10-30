// Exercises the compiled Boost.Contract archive without configuring it:
// each public call checks the class invariant, a precondition and a
// postcondition over an old value, and a precondition failure handler
// installed at run time, in the archive's state, turns a broken
// precondition into an exception.
#include <boost/contract.hpp>
#include <stdexcept>

#ifndef BOOST_CONTRACT_STATIC_LINK
#error "the port's interface definition must select the compiled archive"
#endif

class counter {
    friend class boost::contract::access;
    int value_ = 0;

    void invariant() const { BOOST_CONTRACT_ASSERT(value_ >= 0); }

public:
    void add(int n) {
        boost::contract::old_ptr<int> old = BOOST_CONTRACT_OLDOF(value_);
        boost::contract::check c = boost::contract::public_function(this)
            .precondition([&] { BOOST_CONTRACT_ASSERT(n > 0); })
            .postcondition([&] { BOOST_CONTRACT_ASSERT(value_ == *old + n); });
        value_ += n;
    }

    int value() const {
        boost::contract::check c = boost::contract::public_function(this);
        return value_;
    }
};

int main() {
    boost::contract::set_precondition_failure(
        [](boost::contract::from) { throw std::invalid_argument("precondition"); });

    counter c;
    c.add(2);
    c.add(3);
    if (c.value() != 5) return 1;
    try {
        c.add(-1);
        return 2;
    } catch (const std::invalid_argument&) {
    }
    return c.value() == 5 ? 0 : 3;
}
