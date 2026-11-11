// The package's build script.  Cabin compiles it for the machine the
// build runs on and runs it before it compiles the package.  The script
// writes `build_info.h` into CABIN_OUT_DIR and reports, through
// CABIN_RESULT_FILE, that the target takes that directory as an include
// directory.

// Quiets MSVC's advice to use its own `_s` functions.
#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    const char *out_dir = getenv("CABIN_OUT_DIR");
    char path[4096];
    snprintf(path, sizeof path, "%s/build_info.h", out_dir);
    FILE *header = fopen(path, "w");
    if (!header) {
        perror(path);
        return 1;
    }
    fprintf(header, "#define BUILD_PROFILE \"%s\"\n", getenv("CABIN_PROFILE"));
    fprintf(header, "#define BUILD_TARGET_OS \"%s\"\n", getenv("CABIN_TARGET_OS"));
    if (fclose(header) != 0) {
        return 1;
    }

    FILE *result = fopen(getenv("CABIN_RESULT_FILE"), "w");
    if (!result) {
        return 1;
    }
    fprintf(result, "include-dir build-script-header %s\n", out_dir);
    // Without this line the script runs again whenever a file of the
    // package changes; with it, only when `build.c` does.
    fprintf(result, "rerun-if-changed %s/build.c\n", getenv("CABIN_MANIFEST_DIR"));
    return fclose(result) != 0;
}
