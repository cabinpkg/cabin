// `embed <file>` writes the file's bytes to standard output as a C
// array named `embedded`, with a zero after the last byte.

#include <stdio.h>

int main(int argc, char **argv) {
    FILE *input;
    if (argc != 2 || !(input = fopen(argv[1], "rb"))) {
        fputs("usage: embed <file>\n", stderr);
        return 1;
    }
    puts("static const unsigned char embedded[] = {");
    for (int byte; (byte = fgetc(input)) != EOF;) {
        printf("%d,", byte);
    }
    puts("0};");
    // Flushed, not closed, to sidestep a Windows limit: a program a build
    // script starts with `_spawnl` can fail as it exits once it has closed
    // its standard output (docs/build-scripts.md, "Confinement").
    return ferror(input) || fflush(stdout) != 0;
}
