// Exercises Boost.Predef: version-number encoding, and that the C++ language, a compiler,
// and an architecture word size are detected on the build host.
#include <boost/predef.h>

#if BOOST_VERSION_NUMBER(2, 3, 4) != 20300004
#error "BOOST_VERSION_NUMBER encoding changed"
#endif
#if !BOOST_LANG_STDCPP
#error "C++ language not detected"
#endif
#if !(BOOST_COMP_CLANG || BOOST_COMP_GNUC || BOOST_COMP_MSVC)
#error "compiler not detected"
#endif

int main() {
    if (BOOST_VERSION_NUMBER_MAJOR(BOOST_VERSION_NUMBER(2, 3, 4)) != 2) return 1;
    if (BOOST_VERSION_NUMBER_MINOR(BOOST_VERSION_NUMBER(2, 3, 4)) != 3) return 1;
    if (BOOST_VERSION_NUMBER_PATCH(BOOST_VERSION_NUMBER(2, 3, 4)) != 4) return 1;
    if (BOOST_LANG_STDCPP < BOOST_PREDEF_MAKE_YYYYMM(199711L)) return 1;
    if (BOOST_ARCH_WORD_BITS != sizeof(void*) * 8) return 1;
    return 0;
}
