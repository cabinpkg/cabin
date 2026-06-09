#include <stdio.h>
#include <xxhash.h>

int main(void) {
    unsigned version = XXH_versionNumber();
    printf("fake xxhash: %u\n", version);
    return version == 0u;
}
