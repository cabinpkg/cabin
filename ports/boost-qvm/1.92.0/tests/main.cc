// Exercises Boost.QVM: vector and matrix types with element access, dot and cross products,
// magnitude, and matrix-vector multiplication.
#include <boost/qvm.hpp>

using namespace boost::qvm;

int main() {
    vec<float, 3> a = {{1, 2, 3}};
    vec<float, 3> b = {{4, 5, 6}};
    if (X(a) != 1 || Y(a) != 2 || Z(a) != 3) return 1;
    if (dot(a, b) != 32) return 1;

    vec<float, 3> c = cross(a, b);
    if (X(c) != -3 || Y(c) != 6 || Z(c) != -3) return 1;

    vec<float, 3> s = a + b;
    if (Z(s) != 9) return 1;

    vec<float, 2> d = {{3, 4}};
    if (mag(d) != 5) return 1;

    mat<float, 3, 3> m = diag_mat(b);
    vec<float, 3> p = m * a;
    if (X(p) != 4 || Y(p) != 10 || Z(p) != 18) return 1;

    mat<float, 3, 3> id = identity_mat<float, 3>();
    if (A<1, 1>(id) != 1 || A<0, 1>(id) != 0) return 1;
    if (!(id * a == a)) return 1;
    return 0;
}
