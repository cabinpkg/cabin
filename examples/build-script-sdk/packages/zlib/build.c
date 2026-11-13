// The package's build script.  It decides where the library `z` comes
// from.
//
// With the `vendored` feature it reports nothing, and Cabin builds the
// sources beside this file.  Without it the library is the system's:
// the SDK in the directory CABIN_SDK_ZLIB_DIR names when that is set,
// and otherwise the module `zlib` that pkg-config knows.  A system zlib
// that is missing or too old fails the build: the script never switches
// to the sources by itself.

// Quiets MSVC's advice to use its own `_s` functions.
#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// The oldest system zlib this package accepts, as a version and as the
// `ZLIB_VERNUM` its header defines.
#define OLDEST "1.2.11"
#define OLDEST_VERNUM 0x12b0

static int fail(const char *problem, const char *subject) {
    fprintf(stderr, "zlib: %s%s\n", problem, subject);
    fputs("zlib: set CABIN_SDK_ZLIB_DIR to the absolute path of a directory with zlib " OLDEST
          " or newer (include/zlib.h and its library under lib/), or enable the `vendored` "
          "feature\n",
          stderr);
    return 1;
}

// `ZLIB_VERNUM` as the header at `path` defines it, or 0.
static unsigned long vernum(const char *path) {
    const char define[] = "#define ZLIB_VERNUM ";
    char line[512];
    unsigned long found = 0;
    FILE *file = fopen(path, "r");
    if (!file) {
        return 0;
    }
    while (!found && fgets(line, sizeof line, file)) {
        if (strncmp(line, define, sizeof define - 1) == 0) {
            found = strtoul(line + sizeof define - 1, NULL, 16);
        }
    }
    fclose(file);
    return found;
}

static int exists(const char *path) {
    FILE *file = fopen(path, "rb");
    if (!file) {
        return 0;
    }
    fclose(file);
    return 1;
}

int main(void) {
    const char *sdk = getenv("CABIN_SDK_ZLIB_DIR");
    int msvc = strcmp(getenv("CABIN_TARGET_DIALECT"), "msvc") == 0;
    // A static library first: a program linked with one needs no file
    // when it runs.
    static const char *const gnu[] = {"libz.a", "libz.so", "libz.dylib", "libz.tbd", NULL};
    static const char *const lib[] = {"zlibstatic.lib", "zs.lib", "zlib.lib", "z.lib", NULL};
    const char *const *name = msvc ? lib : gnu;
    char header[4096], library[4096];
    FILE *result;

    if (getenv("CABIN_FEATURE_VENDORED")) {
        return 0;
    }
    if (!sdk && msvc) {
        return fail("pkg-config does not describe libraries for an MSVC-style build", "");
    }
    if (sdk) {
        snprintf(header, sizeof header, "%s/include/zlib.h", sdk);
        if (!exists(header)) {
            return fail("there is no ", header);
        }
        if (vernum(header) < OLDEST_VERNUM) {
            return fail("older than " OLDEST ": ", header);
        }
        for (; *name; name++) {
            snprintf(library, sizeof library, "%s/lib/%s", sdk, *name);
            if (exists(library)) {
                break;
            }
        }
        if (!*name) {
            return fail("there is no zlib library under lib/ of ", sdk);
        }
    }

    if (!(result = fopen(getenv("CABIN_RESULT_FILE"), "w"))) {
        return 1;
    }
    // Cabin builds nothing of the target: the lines after this one say
    // what stands for it.
    fprintf(result, "external z\n");
    if (sdk) {
        fprintf(result, "include-dir z %s/include\n", sdk);
        fprintf(result, "link-file z %s\n", library);
        // Run again when the SDK changes, and not when the sources
        // beside this file do.
        fprintf(result, "rerun-if-changed %s\n", header);
        fprintf(result, "rerun-if-changed %s\n", library);
    } else {
        // Cabin asks pkg-config, and fails the build when the module is
        // missing or older.
        fprintf(result, "pkg-config z zlib >=" OLDEST "\n");
    }
    return fclose(result) != 0;
}
