// Exercises Boost.Parameter: a function with named required and optional parameters,
// called positionally, by name, and in reordered form.
#include <boost/parameter/name.hpp>
#include <boost/parameter/preprocessor.hpp>

BOOST_PARAMETER_NAME(base)
BOOST_PARAMETER_NAME(scale)
BOOST_PARAMETER_NAME(offset)

BOOST_PARAMETER_FUNCTION((int), compute, tag,
    (required (base, (int)))
    (optional (scale, (int), 1) (offset, (int), 0)))
{
    // Named parameters are bound as locals; the same values are reachable through the pack `args`.
    if (args[_base] != base) return -1;
    return base * scale + offset;
}

int main() {
    if (compute(5) != 5) return 1;
    if (compute(5, 3) != 15) return 1;
    if (compute(_base = 5, _offset = 2) != 7) return 1;
    if (compute(_offset = 1, _scale = 4, _base = 2) != 9) return 1;
    return 0;
}
