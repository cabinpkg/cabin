#include <stdio.h>
#include <string.h>
#include <xxhash.h>

int main(void) {
    const char *data = "Cabin";
    XXH64_hash_t digest = XXH64(data, strlen(data), /*seed=*/0);
    printf("xxHash version: %u\n", XXH_versionNumber());
    printf("XXH64(\"Cabin\") = %016llx\n", (unsigned long long)digest);
    return 0;
}
