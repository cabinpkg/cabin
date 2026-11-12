// The package's build script.  It packs `assets/banner.txt` with zlib
// into a header, and the program unpacks it when it runs.  The script
// runs on the build machine, so the zlib it links is the package's
// build dependency, not the zlib the program is linked with.

// Quiets MSVC's advice to use its own `_s` functions.
#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
#include <zlib.h>

int main(void) {
    const char *out_dir = getenv("CABIN_OUT_DIR");
    const char *manifest_dir = getenv("CABIN_MANIFEST_DIR");
    static Bytef text[4096], packed[8192];
    uLongf packed_len = sizeof packed;
    char header[4096];

    // The script starts in the package's root.
    FILE *file = fopen("assets/banner.txt", "rb");
    if (!file) {
        return 1;
    }
    uLong len = (uLong)fread(text, 1, sizeof text, file);
    // A banner that fills the buffer may not have fit it.
    int unread = ferror(file) || len == sizeof text;
    fclose(file);
    if (unread || compress(packed, &packed_len, text, len) != Z_OK) {
        return 1;
    }

    snprintf(header, sizeof header, "%s/banner.h", out_dir);
    if (!(file = fopen(header, "w"))) {
        return 1;
    }
    fprintf(file, "#define BANNER_PACKED_BY \"%s\"\n", zlibVersion());
    fprintf(file, "#define BANNER_LEN %lu\n", (unsigned long)len);
    fprintf(file, "static const unsigned char banner[] = {");
    for (uLongf i = 0; i < packed_len; i++) {
        fprintf(file, "%u,", packed[i]);
    }
    fprintf(file, "};\n");
    if (fclose(file) != 0) {
        return 1;
    }

    if (!(file = fopen(getenv("CABIN_RESULT_FILE"), "w"))) {
        return 1;
    }
    fprintf(file, "include-dir app %s\n", out_dir);
    fprintf(file, "rerun-if-changed %s/build.c\n", manifest_dir);
    fprintf(file, "rerun-if-changed %s/assets/banner.txt\n", manifest_dir);
    return fclose(file) != 0;
}
