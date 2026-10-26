// Exercises Boost.GIL: typed images and views, pixel algorithms, view
// adaptors, color and channel-depth conversion, a runtime-typed
// any_image, and an in-memory BMP round trip through the io extension.
#include <boost/gil.hpp>
#include <boost/gil/extension/dynamic_image/any_image.hpp>
#include <boost/gil/extension/io/bmp.hpp>

#include <sstream>
#include <type_traits>

namespace gil = boost::gil;

int main() {
    gil::rgb8_image_t img(4, 3);
    const gil::rgb8_view_t v = gil::view(img);
    gil::fill_pixels(v, gil::rgb8_pixel_t(10, 20, 30));
    v(1, 2) = gil::rgb8_pixel_t(255, 0, 0);
    if (gil::get_color(v(0, 0), gil::green_t()) != 20) return 1;
    if (gil::flipped_up_down_view(v)(1, 0) != v(1, 2)) return 1;
    if (gil::rotated90cw_view(v).width() != 3 || gil::subimage_view(v, 1, 1, 2, 2)(0, 1) != v(1, 2)) return 1;

    gil::gray8_image_t gray(v.dimensions());
    gil::copy_and_convert_pixels(v, gil::view(gray));
    const gil::gray8c_view_t g = gil::const_view(gray);
    if (g(0, 0) == g(1, 2) || g(0, 0) != g(3, 1)) return 1;

    gil::rgb16_image_t deep(v.dimensions());
    gil::copy_and_convert_pixels(v, gil::view(deep));
    if (gil::view(deep)(1, 2) != gil::rgb16_pixel_t(65535, 0, 0)) return 1;

    gil::any_image<gil::gray8_image_t, gil::rgb8_image_t> dynamic(gil::rgb8_image_t(2, 2));
    const auto channels = boost::variant2::visit([](const auto& view) {
        return static_cast<int>(gil::num_channels<std::decay_t<decltype(view)>>::value);
    }, gil::view(dynamic));
    if (channels != 3) return 1;

    std::stringstream bmp(std::ios::in | std::ios::out | std::ios::binary);
    gil::write_view(bmp, gil::const_view(img), gil::bmp_tag());
    gil::rgb8_image_t back;
    gil::read_image(bmp, back, gil::bmp_tag());
    return back.dimensions() == img.dimensions() && gil::equal_pixels(gil::const_view(back), gil::const_view(img))
               ? 0
               : 1;
}
