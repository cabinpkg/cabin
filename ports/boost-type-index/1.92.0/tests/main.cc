// Boost.TypeIndex: compare type indices with and without cv/ref qualifiers, recover the dynamic
// type of a polymorphic object, and get a human-readable type name.
#include <boost/type_index.hpp>
#include <string>

struct Base { virtual ~Base() {} };
struct Derived : Base {};

int main() {
    using boost::typeindex::type_id;
    using boost::typeindex::type_id_with_cvr;
    using boost::typeindex::type_id_runtime;

    if (type_id<int>() != type_id<const int&>()) return 1;
    if (type_id<int>() == type_id<long>()) return 1;
    if (type_id_with_cvr<int>() == type_id_with_cvr<const int>()) return 1;
    if (type_id<int>().hash_code() != type_id<int>().hash_code()) return 1;

    Derived d;
    const Base& b = d;
    if (type_id_runtime(b) != type_id<Derived>()) return 1;
    if (type_id_runtime(b) == type_id<Base>()) return 1;

    return type_id<int>().pretty_name() == "int" ? 0 : 1;
}
