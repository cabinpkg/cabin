#include <stdio.h>
#include <zlib.h>

int main(void) {
    const char *version = zlibVersion();
    printf("fake zlib: %s\n", version);
    return version[0] == '\0';
}
