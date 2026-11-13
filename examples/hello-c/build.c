#define _POSIX_C_SOURCE 200809L
#define _CRT_SECURE_NO_WARNINGS
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <process.h>
#else
#include <sys/wait.h>
#include <unistd.h>
#endif

/* Runs `program` with `argv` and waits for it, without a shell. */
static int run(const char *program, const char *const argv[]) {
#ifdef _WIN32
    /* `_spawnv` passes the arguments on unquoted. */
    char quoted[8][4200];
    const char *args[8];
    size_t count = 0;
    for (; argv[count]; count++) {
        snprintf(quoted[count], sizeof quoted[count], "\"%s\"", argv[count]);
        args[count] = quoted[count];
    }
    args[count] = NULL;
    return (int)_spawnv(_P_WAIT, program, args);
#else
    int status = 0;
    pid_t child = fork();
    if (child == 0) {
        execv(program, (char *const *)argv);
        _exit(127);
    }
    if (child < 0 || waitpid(child, &status, 0) != child || !WIFEXITED(status)) return -1;
    return WEXITSTATUS(status);
#endif
}

int main(void) {
    const char *out = getenv("CABIN_OUT_DIR");
    const char *cc = getenv("CABIN_CC");
    const char *ar = getenv("CABIN_AR");
    int msvc = strcmp(getenv("CABIN_TARGET_DIALECT"), "msvc") == 0;
    char object[4096], library[4096], object_flag[4100], library_flag[4100];
    FILE *file;
    if (!cc || !ar) return 1;
    snprintf(object, sizeof object, "%s/greet.%s", out, msvc ? "obj" : "o");
    snprintf(library, sizeof library, "%s/%s", out, msvc ? "greet.lib" : "libgreet.a");
    snprintf(object_flag, sizeof object_flag, "/Fo%s", object);
    snprintf(library_flag, sizeof library_flag, "/OUT:%s", library);
    {
        /* `/Zl` names no C runtime, so the object links with any. */
        const char *const cl[] = {"cc", "/nologo", "/Zl", "/c", "vendor/greet.c", object_flag, NULL};
        const char *const lib[] = {"ar", "/nologo", library_flag, object, NULL};
        const char *const compile[] = {"cc", "-c", "vendor/greet.c", "-o", object, NULL};
        const char *const archive[] = {"ar", "rcs", library, object, NULL};
        if (run(cc, msvc ? cl : compile) != 0 || run(ar, msvc ? lib : archive) != 0) return 1;
    }
    if (!(file = fopen(getenv("CABIN_RESULT_FILE"), "w"))) return 1;
    fprintf(file, "external greet\nlink-file greet %s\n", library);
    return fclose(file) != 0;
}
