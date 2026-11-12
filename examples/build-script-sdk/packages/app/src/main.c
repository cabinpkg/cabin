// Uses zlib only through `packer.h`: `packer` declares zlib a public
// dependency, so this file compiles with zlib's header and the program
// links zlib's library.
#include <banner.h>
#include <packer.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    // Unpack what the build script packed.
    Bytef text[BANNER_LEN + 1] = {0};
    uLongf len = BANNER_LEN;
    if (uncompress(text, &len, banner, sizeof banner) != Z_OK || len != BANNER_LEN) {
        return 1;
    }

    // Pack and unpack it once more, with this program's zlib alone.
    Bytef packed[BANNER_LEN + 64], again[BANNER_LEN];
    uLongf packed_len = sizeof packed;
    if (packer_pack(packed, &packed_len, text, len) != Z_OK
        || uncompress(again, &len, packed, packed_len) != Z_OK
        || memcmp(again, text, BANNER_LEN) != 0) {
        return 1;
    }

    printf("%s", (const char *)text);
    printf("packed by zlib %s, unpacked by zlib %s (header %s)\n", BANNER_PACKED_BY, packer_zlib(),
           ZLIB_VERSION);
    return 0;
}
