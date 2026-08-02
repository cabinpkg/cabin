#include <sqlite3.h>
#include <stdio.h>

int main(void) {
    printf("fake sqlite3: %s\n", sqlite3_libversion());
    printf("fake sqlite3 threadsafe: %d\n", sqlite3_threadsafe());
    return sqlite3_threadsafe() != 0;
}
