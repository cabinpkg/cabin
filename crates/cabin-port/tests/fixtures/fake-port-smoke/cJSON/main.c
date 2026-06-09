#include <cJSON.h>
#include <stdio.h>

int main(void) {
    const char *version = cJSON_Version();
    printf("fake cJSON: %s\n", version);
    return version[0] == '\0';
}
