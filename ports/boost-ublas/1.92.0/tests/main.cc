// Exercises Boost.uBLAS's dense and sparse expression templates: matrix
// and vector products, transposes, norms and inner products, an LU
// factorization with partial pivoting that solves a linear system, and
// a compressed sparse matrix-vector product, all with exact results.
#include <boost/numeric/ublas/lu.hpp>
#include <boost/numeric/ublas/matrix.hpp>
#include <boost/numeric/ublas/matrix_sparse.hpp>
#include <boost/numeric/ublas/vector.hpp>

namespace ublas = boost::numeric::ublas;

int main() {
    ublas::matrix<double> a(3, 3);
    const double values[3][3] = {{0, 2, 1}, {1, 1, 1}, {2, 1, 3}};
    for (unsigned i = 0; i < 3; ++i)
        for (unsigned j = 0; j < 3; ++j) a(i, j) = values[i][j];
    ublas::vector<double> x(3);
    x(0) = 1;
    x(1) = -2;
    x(2) = 3;

    const ublas::vector<double> b = ublas::prod(a, x);
    if (b(0) != -1 || b(1) != 2 || b(2) != 9) return 1;
    const ublas::matrix<double> ata = ublas::prod(ublas::trans(a), a);
    if (ata(0, 0) != 5 || ata(1, 2) != 6 || ata(2, 1) != 6) return 2;
    if (ublas::inner_prod(x, x) != 14 || ublas::norm_1(x) != 6 || ublas::norm_inf(b) != 9) return 3;

    // Solve a * y = b by LU; row 0 has a zero pivot, so it must pivot.
    ublas::matrix<double> lu = a;
    ublas::permutation_matrix<std::size_t> pm(3);
    if (ublas::lu_factorize(lu, pm) != 0) return 4;
    ublas::vector<double> y = b;
    ublas::lu_substitute(lu, pm, y);
    if (ublas::norm_inf(y - x) > 1e-12) return 5;

    ublas::compressed_matrix<int> sparse(1000, 1000);
    sparse(0, 999) = 4;
    sparse(500, 0) = -3;
    sparse(999, 500) = 7;
    ublas::vector<int> ones(1000);
    for (std::size_t i = 0; i < ones.size(); ++i) ones(i) = 1;
    const ublas::vector<int> s = ublas::prod(sparse, ones);
    return sparse.nnz() == 3 && s(0) == 4 && s(500) == -3 && s(999) == 7 && s(1) == 0 ? 0 : 6;
}
