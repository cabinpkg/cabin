#include <cstdio>
#include <tinyxml2.h>

int main() {
    tinyxml2::XMLDocument doc;
    if (doc.Parse("<note><to>Cabin</to></note>") != tinyxml2::XML_SUCCESS) {
        std::fprintf(stderr, "tinyxml2 parse failed\n");
        return 1;
    }

    const tinyxml2::XMLElement *note = doc.FirstChildElement("note");
    const tinyxml2::XMLElement *to = note ? note->FirstChildElement("to") : nullptr;
    const char *text = to ? to->GetText() : nullptr;

    std::printf("tinyxml2 parsed to: %s\n", text ? text : "(null)");
    std::printf("tinyxml2 version: %d.%d.%d\n", TIXML2_MAJOR_VERSION,
                TIXML2_MINOR_VERSION, TIXML2_PATCH_VERSION);
    return 0;
}
