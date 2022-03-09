include_guard(GLOBAL)

message(CHECK_START "Adding Toml11")
list(APPEND CMAKE_MESSAGE_INDENT "  ")

FetchContent_Declare(
        toml11
        GIT_REPOSITORY https://github.com/ToruNiina/toml11.git
        GIT_TAG        bf2384d8da9cdd91bdc51092452e02cc982af0c7
)

set(CMAKE_PROJECT_toml11_INCLUDE_BEFORE "${CMAKE_SOURCE_DIR}/cmake/CMP0077PolicyFix.cmake")
set(toml11_BUILD_TEST OFF)
# This seems GCC's bug
if (CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
    add_compile_options(-Wno-switch-enum)
endif ()
FetchContent_MakeAvailable(toml11)

list(APPEND POAC_DEPENDENCIES toml11::toml11)
message(CHECK_PASS "added")

list(POP_BACK CMAKE_MESSAGE_INDENT)
