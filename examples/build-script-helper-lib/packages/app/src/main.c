#include <stdio.h>
#include <string.h>

// Defined by `crc32_table.c`, which the build script generates.
extern const unsigned long crc32_table[256];

static unsigned long crc32(const char *text) {
    unsigned long crc = 0xFFFFFFFFUL;
    for (size_t index = 0; index < strlen(text); ++index) {
        crc = crc32_table[(crc ^ (unsigned char)text[index]) & 0xFFUL] ^ (crc >> 8);
    }
    return crc ^ 0xFFFFFFFFUL;
}

int main(void) {
    printf("crc32(\"cabin\") = %08lx\n", crc32("cabin"));
    return 0;
}
