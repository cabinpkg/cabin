#include <stdio.h>

#include "build_info.h"

int main(void) {
    printf("built with the %s profile for %s\n", BUILD_PROFILE, BUILD_TARGET_OS);
    return 0;
}
