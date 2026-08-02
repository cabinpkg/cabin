#include "sqlite3.h"

#ifndef SQLITE_THREADSAFE
#define SQLITE_THREADSAFE 1
#endif

const char *sqlite3_libversion(void) { return "3.53.2"; }

int sqlite3_threadsafe(void) { return SQLITE_THREADSAFE; }
