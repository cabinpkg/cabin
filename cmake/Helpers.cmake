# ref: https://stackoverflow.com/a/7788165
macro (list_dir_items result curdir)
    file(GLOB items RELATIVE ${curdir} ${curdir}/*)
    set(${result} ${items})
endmacro ()

macro (get_homebrew_prefix_path)
    execute_process(
        COMMAND brew --prefix
        OUTPUT_VARIABLE POAC_HOMEBREW_PREFIX_PATH
        OUTPUT_STRIP_TRAILING_WHITESPACE
    )
endmacro ()

if (APPLE)
    get_homebrew_prefix_path()
    set(POAC_HOMEBREW_ROOT_PATH "${POAC_HOMEBREW_PREFIX_PATH}/opt")
endif()
