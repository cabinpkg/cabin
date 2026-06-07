#include <cJSON.h>
#include <stdio.h>

int main(void) {
    const char *text = "{\"name\":\"Cabin\",\"stars\":5}";
    cJSON *root = cJSON_Parse(text);
    if (root == NULL) {
        fprintf(stderr, "cJSON parse failed\n");
        return 1;
    }

    const cJSON *name = cJSON_GetObjectItemCaseSensitive(root, "name");
    if (cJSON_IsString(name) && name->valuestring != NULL) {
        printf("cJSON parsed name: %s\n", name->valuestring);
    }
    printf("cJSON version: %s\n", cJSON_Version());

    cJSON_Delete(root);
    return 0;
}
