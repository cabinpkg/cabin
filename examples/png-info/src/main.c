#include <png.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <zlib.h>

/* Encode a tiny RGBA image to an in-memory PNG with libpng's
 * simplified write API, then decode it back and report what a
 * png-info tool would print. The DEFLATE stream inside the PNG is
 * produced and consumed by zlib, which this package never declares:
 * it arrives transitively through the libpng port edge. */

int main(void) {
    static const unsigned char pixels[2 * 2 * 4] = {
        255, 0,   0,   255, /* red */
        0,   255, 0,   255, /* green */
        0,   0,   255, 255, /* blue */
        255, 255, 255, 255, /* white */
    };

    png_image out;
    memset(&out, 0, sizeof out);
    out.version = PNG_IMAGE_VERSION;
    out.width = 2;
    out.height = 2;
    out.format = PNG_FORMAT_RGBA;

    png_alloc_size_t size = 0;
    if (!png_image_write_to_memory(&out, NULL, &size, 0, pixels, 0, NULL)) {
        fprintf(stderr, "sizing failed: %s\n", out.message);
        return 1;
    }
    void *encoded = malloc(size);
    if (encoded == NULL
        || !png_image_write_to_memory(&out, encoded, &size, 0, pixels, 0,
                                      NULL)) {
        fprintf(stderr, "encode failed: %s\n", out.message);
        free(encoded);
        return 1;
    }
    png_image_free(&out);

    png_image in;
    memset(&in, 0, sizeof in);
    in.version = PNG_IMAGE_VERSION;
    if (!png_image_begin_read_from_memory(&in, encoded, size)) {
        fprintf(stderr, "decode failed: %s\n", in.message);
        free(encoded);
        return 1;
    }
    printf("png-info: %ux%u, %u channel(s), %llu byte(s) encoded\n",
           (unsigned)in.width, (unsigned)in.height,
           (unsigned)PNG_IMAGE_PIXEL_CHANNELS(in.format),
           (unsigned long long)size);

    unsigned char decoded[sizeof pixels];
    in.format = PNG_FORMAT_RGBA;
    if (!png_image_finish_read(&in, NULL, decoded, 0, NULL)) {
        fprintf(stderr, "read failed: %s\n", in.message);
        free(encoded);
        return 1;
    }
    free(encoded);
    printf("roundtrip pixels match: %s\n",
           memcmp(pixels, decoded, sizeof pixels) == 0 ? "yes" : "no");
    printf("libpng version: %s\n", PNG_LIBPNG_VER_STRING);
    printf("zlib version (transitive port edge): %s\n", zlibVersion());
    return 0;
}
