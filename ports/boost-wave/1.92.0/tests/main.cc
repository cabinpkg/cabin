// Exercises Boost.Wave's compiled re2c lexer and preprocessor grammars:
// an in-memory translation unit with object-like, function-like,
// stringizing and pasting macros, #if expressions, a macro predefined
// through the context, and a quoted #include resolved from a temporary
// directory, then a malformed directive that must raise cpp_exception.
#include <boost/filesystem.hpp>
#include <boost/wave.hpp>
#include <boost/wave/cpplexer/cpp_lex_iterator.hpp>
#include <boost/wave/cpplexer/cpp_lex_token.hpp>
#include <fstream>
#include <string>

namespace fs = boost::filesystem;
namespace wave = boost::wave;
typedef wave::cpplexer::lex_iterator<wave::cpplexer::lex_token<> > lexer;
typedef wave::context<std::string::iterator, lexer> context;

// Preprocesses `input`, joining the significant tokens with single spaces.
static std::string preprocess(std::string input, const fs::path& include_dir) {
    context ctx(input.begin(), input.end(), "memory.cpp");
    ctx.add_include_path(include_dir.string().c_str());
    ctx.set_language(wave::enable_emit_line_directives(ctx.get_language(), false));
    ctx.add_macro_definition("EXTRA=7");
    std::string out;
    for (context::iterator_type it = ctx.begin(); it != ctx.end(); ++it) {
        const wave::token_id id = wave::token_id(*it);
        if (IS_CATEGORY(id, wave::WhiteSpaceTokenType) || IS_CATEGORY(id, wave::EOLTokenType) ||
            IS_CATEGORY(id, wave::EOFTokenType))
            continue;
        out += (out.empty() ? "" : " ") + std::string(it->get_value().c_str());
    }
    return out;
}

int main() {
    const fs::path dir = fs::temp_directory_path() / fs::unique_path("cabin-wave-%%%%-%%%%-%%%%");
    fs::create_directories(dir);
    std::ofstream((dir / "cabin_wave_probe.h").string().c_str()) << "#define VERSION 3\n";

    const std::string got = preprocess("#define SQUARE(x) ((x) * (x))\n"
                                       "#define CAT(a, b) a ## b\n"
                                       "#define STR(x) #x\n"
                                       "#include \"cabin_wave_probe.h\"\n"
                                       "#if VERSION >= 2 && defined(EXTRA)\n"
                                       "int CAT(my, value) = SQUARE(VERSION) + EXTRA;\n"
                                       "#else\n"
                                       "int wrong;\n"
                                       "#endif\n"
                                       "const char* s = STR(hello  world); int line = __LINE__;\n",
                                       dir);
    bool threw = false;
    try {
        preprocess("#if 1 +\n#endif\n", dir);
    } catch (const wave::cpp_exception&) {
        threw = true;
    }
    fs::remove_all(dir);
    if (got != "int myvalue = ( ( 3 ) * ( 3 ) ) + 7 ; const char * s = \"hello world\" ; int line = 10 ;") return 1;
    return threw ? 0 : 2;
}
