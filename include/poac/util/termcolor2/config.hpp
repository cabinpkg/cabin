#ifndef POAC_UTIL_TERMCOLOR2_CONFIG_HPP_
#define POAC_UTIL_TERMCOLOR2_CONFIG_HPP_

#ifdef __cplusplus
#  ifndef TERMCOLOR2_STD_VER
#    if __cplusplus <= 201703L
#      error \
          "termcolor2 requires a compiler that (fully) implements C++20 or later."
#    else
#      define TERMCOLOR2_STD_VER 20
#    endif
#  endif // !TERMCOLOR2_STD_VER
#else
#  error "__cplusplus is not defined"
#endif // !__cplusplus

//#if defined(__cpp_lib_constexpr_dynamic_alloc) && !defined(__clang__) &&
// TERMCOLOR2_STD_VER > 17
#if false // needs support of constexpr std::string
#  define TERMCOLOR2_CXX20_CONSTEVAL consteval
#  define TERMCOLOR2_CXX20_CONSTEVAL_FN consteval
#  define TERMCOLOR2_CXX20_CONSTINIT constinit
#else
#  define TERMCOLOR2_CXX20_CONSTEVAL
#  define TERMCOLOR2_CXX20_CONSTEVAL_FN constexpr
#  define TERMCOLOR2_CXX20_CONSTINIT
#endif

#ifdef TERMCOLOR2_USE_GNU_STRING_LITERAL_OPERATOR_TEMPLATE
#  ifdef _MSC_VER
//   Does not implement constexpr color literals for MSVC.
#    undef TERMCOLOR2_USE_GNU_STRING_LITERAL_OPERATOR_TEMPLATE
#    ifndef TERMCOLOR2_NO_WARNING
#      pragma message(                                                      \
          "You cannot enable gnu-string-literal-operator-template on MSVC." \
      )
#    endif // !TERMCOLOR2_NO_WARNING
#  elif !defined(TERMCOLOR2_NO_WARNING)
#    warning \
        "gnu-string-literal-operator-template is enabled." \
             "You will see a warning associated with it." \
             "If you want to hide warning, please pass -Wno-gnu-string-literal-operator-template to compiler option."
#  endif // !_MSC_VER
#endif   // !TERMCOLOR2_USE_GNU_STRING_LITERAL_OPERATOR_TEMPLATE

#endif // POAC_UTIL_TERMCOLOR2_CONFIG_HPP_
