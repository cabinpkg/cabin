#include <cstdio>

#include "geometry/geometry.hpp"

int main() {
    std::printf("circle area (r = 2): %.2f\n", geometry::circle_area(2.0));
    std::printf("rectangle area (3 x 4): %.2f\n",
                geometry::rectangle_area(3.0, 4.0));
    return 0;
}
