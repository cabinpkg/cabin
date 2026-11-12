#include <cstdlib>
#include <iostream>
#include <string>
#include <vector>
#include <zlib.h>

int main() {
    const std::string text(200, 'z');
    std::vector<Bytef> packed(compressBound(static_cast<uLong>(text.size())));
    uLongf packed_len = static_cast<uLongf>(packed.size());
    std::string again(text.size(), '\0');
    uLongf len = static_cast<uLongf>(again.size());
    if (compress(packed.data(), &packed_len, reinterpret_cast<const Bytef *>(text.data()),
                 static_cast<uLong>(text.size())) != Z_OK
        || uncompress(reinterpret_cast<Bytef *>(&again[0]), &len, packed.data(), packed_len) != Z_OK
        || again != text) {
        return EXIT_FAILURE;
    }
    std::cout << "c++: " << text.size() << " bytes packed into " << packed_len << " by zlib "
              << zlibVersion() << " (header " << ZLIB_VERSION << ")\n";
    return EXIT_SUCCESS;
}
