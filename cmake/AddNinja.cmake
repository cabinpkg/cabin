include_guard(GLOBAL)

message(CHECK_START "Adding ninja")
list(APPEND CMAKE_MESSAGE_INDENT "  ")

FetchContent_Declare(
        ninja
        GIT_REPOSITORY https://github.com/ninja-build/ninja
        GIT_TAG        v1.11.0
)

set(BUILD_TESTING OFF)
FetchContent_MakeAvailable(ninja)

file(MAKE_DIRECTORY ${ninja_BINARY_DIR}/include)
file(CREATE_LINK ${ninja_SOURCE_DIR}/src ${ninja_BINARY_DIR}/include/ninja SYMBOLIC)

set(NINJA_INCLUDE_DIR ${ninja_BINARY_DIR}/include)
set(NINJA_LIBRARIES libninja libninja-re2c)

list(APPEND POAC_DEPENDENCIES ${NINJA_LIBRARIES})
message(CHECK_PASS "added")

list(POP_BACK CMAKE_MESSAGE_INDENT)
