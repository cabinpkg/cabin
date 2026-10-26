// Exercises Boost.TypeErasure: an any over copying, increment and
// streaming, and dynamic_any_cast, which looks up the bindings that
// register_binding stored in the compiled library's run-time registry.
#include <boost/mpl/vector.hpp>
#include <boost/type_erasure/any.hpp>
#include <boost/type_erasure/any_cast.hpp>
#include <boost/type_erasure/builtin.hpp>
#include <boost/type_erasure/dynamic_any_cast.hpp>
#include <boost/type_erasure/operators.hpp>
#include <boost/type_erasure/register_binding.hpp>
#include <sstream>
#include <string>

namespace te = boost::type_erasure;

typedef boost::mpl::vector<te::copy_constructible<>, te::typeid_<> > base_concept;
typedef boost::mpl::vector<base_concept, te::incrementable<>, te::ostreamable<> > rich_concept;
typedef te::any<base_concept> base_any;
typedef te::any<rich_concept> rich_any;

int main() {
    rich_any x(41);
    ++x;
    std::ostringstream out;
    out << x;
    if (out.str() != "42") return 1;

    te::register_binding<rich_concept, int>();
    rich_any cast = te::dynamic_any_cast<rich_any>(base_any(1));
    ++cast;
    if (te::any_cast<int>(cast) != 2) return 2;

    try {
        te::dynamic_any_cast<rich_any>(base_any(std::string("unregistered")));
        return 3;
    } catch (const te::bad_any_cast&) {
    }
    return 0;
}
