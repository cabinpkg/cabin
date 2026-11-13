#include "packer.h"

int packer_pack(Bytef *to, uLongf *to_len, const Bytef *from, uLong len) {
    return compress(to, to_len, from, len);
}
const char *packer_zlib(void) { return zlibVersion(); }
