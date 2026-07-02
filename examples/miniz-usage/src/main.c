#include <miniz.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    const char *msg = "Cabin compresses with miniz";
    mz_ulong src_len = (mz_ulong)strlen(msg) + 1;
    unsigned char packed[256];
    mz_ulong packed_len = (mz_ulong)sizeof(packed);
    unsigned char unpacked[256];
    mz_ulong unpacked_len = (mz_ulong)sizeof(unpacked);

    if (mz_compress(packed, &packed_len, (const unsigned char *)msg, src_len) != MZ_OK) {
        fprintf(stderr, "mz_compress failed\n");
        return 1;
    }
    if (mz_uncompress(unpacked, &unpacked_len, packed, packed_len) != MZ_OK) {
        fprintf(stderr, "mz_uncompress failed\n");
        return 1;
    }

    printf("miniz roundtrip: %s\n", (const char *)unpacked);
    printf("miniz version: %s\n", mz_version());
    return 0;
}
