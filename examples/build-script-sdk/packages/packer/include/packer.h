#ifndef PACKER_H
#define PACKER_H
// A public header that needs zlib's: what includes this one compiles
// with zlib's include directory too.
#include <zlib.h>

// Compresses `len` bytes at `from` into `to`, which holds `*to_len`.
int packer_pack(Bytef *to, uLongf *to_len, const Bytef *from, uLong len);
// The zlib this library was linked with.
const char *packer_zlib(void);
#endif
