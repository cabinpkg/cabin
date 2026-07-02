#pragma once

// Header-only: every function is defined in the header, so there is
// no archive to build or link - dependents only need the include dir.
namespace geometry {

inline constexpr double kPi = 3.14159265358979323846;

inline constexpr double circle_area(double radius) {
    return kPi * radius * radius;
}

inline constexpr double rectangle_area(double width, double height) {
    return width * height;
}

}  // namespace geometry
