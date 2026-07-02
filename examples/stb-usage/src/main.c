/* stb headers are single-file libraries: exactly one translation
 * unit defines the implementation macro before the include and
 * thereby hosts the function bodies. */
#define STB_SPRINTF_IMPLEMENTATION
#include <stb_sprintf.h>

#include <stdio.h>

int main(void) {
    char buf[64];
    stbsp_snprintf(buf, (int)sizeof(buf), "%s scores %d", "Cabin", 42);
    printf("stb_sprintf formatted: %s\n", buf);
    return 0;
}
