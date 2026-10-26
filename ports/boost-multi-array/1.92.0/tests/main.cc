// Exercises boost::multi_array: shape, element access, a sub-array view, and reshape.
#include <boost/multi_array.hpp>

int main() {
    typedef boost::multi_array<int, 2> grid;
    grid a(boost::extents[3][4]);
    if (a.num_dimensions() != 2 || a.shape()[0] != 3 || a.shape()[1] != 4) return 1;
    if (a.num_elements() != 12) return 1;

    for (grid::index i = 0; i != 3; ++i)
        for (grid::index j = 0; j != 4; ++j) a[i][j] = static_cast<int>(i * 10 + j);
    if (a[2][3] != 23) return 1;

    typedef grid::index_range range;
    grid::array_view<2>::type view = a[boost::indices[range(1, 3)][range(0, 4, 2)]];
    if (view.shape()[0] != 2 || view.shape()[1] != 2) return 1;
    if (view[1][1] != 22) return 1;

    boost::array<grid::index, 2> dims = {{4, 3}};
    a.reshape(dims);
    if (a.shape()[0] != 4 || a[1][0] != 3) return 1;
    return 0;
}
