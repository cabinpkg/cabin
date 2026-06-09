#include "png.h"
#include "pnglibconf.h"
#include <zlib.h>

unsigned long png_access_version_number(void) { return FAKE_PNGLIBCONF_VERSION; }

const char *png_fake_zlib_version(void) { return zlibVersion(); }
