#include <png.h>
#include <stdio.h>
#include <zlib.h>

int main(void) {
    // Force a real libpng symbol: if the libpng archive were not
    // linked, this call would fail at link time.
    png_structp png =
        png_create_read_struct(PNG_LIBPNG_VER_STRING, NULL, NULL, NULL);
    if (png == NULL) {
        fprintf(stderr, "failed to create libpng read struct\n");
        return 1;
    }
    printf("libpng version: %s\n", png_get_libpng_ver(png));
    png_destroy_read_struct(&png, NULL, NULL);

    // zlib.h reaches this target only through its own zlib edge:
    // libpng's edge onto zlib is private.
    printf("zlib version: %s\n", zlibVersion());
    return 0;
}
