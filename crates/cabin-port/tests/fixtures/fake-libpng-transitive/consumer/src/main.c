#include <png.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    const char *zlib_version = png_fake_zlib_version();
    printf("fake libpng version: %lu\n", png_access_version_number());
    printf("fake zlib via libpng: %s\n", zlib_version);
    if (png_access_version_number() != 10650UL) return 1;
    if (strcmp(zlib_version, "fake-zlib/1.3.1") != 0) return 1;
    return 0;
}
