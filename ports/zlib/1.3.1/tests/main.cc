// Round-trips a buffer through compress2/uncompress and checks the
// linked library reports the version the headers were built against.
#include <zlib.h>

#include <cstdio>
#include <cstring>

int main() {
    if (std::strcmp(zlibVersion(), ZLIB_VERSION) != 0) {
        std::fprintf(stderr, "header %s, library %s\n", ZLIB_VERSION, zlibVersion());
        return 1;
    }
    const char text[] = "Cabin consumes cabin-ports/zlib from the registry.";
    unsigned char packed[128];
    uLongf packed_len = sizeof packed;
    if (compress2(packed, &packed_len, reinterpret_cast<const Bytef*>(text), sizeof text,
                  Z_BEST_COMPRESSION) != Z_OK) {
        return 1;
    }
    char unpacked[sizeof text];
    uLongf unpacked_len = sizeof unpacked;
    if (uncompress(reinterpret_cast<Bytef*>(unpacked), &unpacked_len, packed, packed_len) != Z_OK) {
        return 1;
    }
    return unpacked_len == sizeof text && std::memcmp(unpacked, text, sizeof text) == 0 ? 0 : 1;
}
