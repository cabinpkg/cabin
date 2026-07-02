#include <stdio.h>
#include <string.h>
#include <uthash.h>

/* Stringize the bare `2.4.0` token UTHASH_VERSION expands to. */
#define STR2(x) #x
#define STR(x) STR2(x)

struct word {
    char name[16];
    int count;
    UT_hash_handle hh;
};

int main(void) {
    struct word *table = NULL;

    struct word cabin;
    memset(&cabin, 0, sizeof(cabin));
    snprintf(cabin.name, sizeof(cabin.name), "cabin");
    cabin.count = 42;
    HASH_ADD_STR(table, name, &cabin);

    struct word *found = NULL;
    HASH_FIND_STR(table, "cabin", found);
    if (found == NULL) {
        fprintf(stderr, "uthash lookup failed\n");
        return 1;
    }

    printf("uthash lookup: %s = %d\n", found->name, found->count);
    printf("uthash version: %s\n", STR(UTHASH_VERSION));
    return 0;
}
