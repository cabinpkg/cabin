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

    // zlibVersion() is a real zlib symbol reached only through the
    // transitive libpng -> zlib port edge. Both the header (zlib.h)
    // and the archive must propagate through libpng for this to
    // compile and link, so calling it proves the transitive edge.
    printf("zlib version (via libpng port edge): %s\n", zlibVersion());
    return 0;
}
