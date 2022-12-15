#pragma once

// internal
#include "poac/util/cfg.hpp" // compiler
#include "poac/util/format.hpp"
#include "poac/util/log.hpp"
#include "poac/util/result.hpp"
#include "poac/util/rustify.hpp"

namespace poac::core::builder::compiler::lang {

enum class Lang {
  c,
  cxx,
};

Fn to_string(Lang lang)->String;

} // namespace poac::core::builder::compiler::lang

namespace fmt {

template <>
struct formatter<poac::core::builder::compiler::lang::Lang> {
  static constexpr Fn parse(format_parse_context& ctx) { return ctx.begin(); }

  template <typename FormatContext>
  inline Fn
  format(poac::core::builder::compiler::lang::Lang l, FormatContext& ctx) {
    return format_to(
        ctx.out(), "{}", poac::core::builder::compiler::lang::to_string(l)
    );
  }
};

} // namespace fmt
