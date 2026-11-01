//! Refuse a registry package whose include dir would replace a
//! standard header in the compiles that depend on it.
//!
//! GCC, Clang, clang-cl, and MSVC all search `-I` / `-isystem` /
//! `/I` / `/external:I` dirs before their own standard headers, so a
//! file named `stdint.h` or `version` directly inside a dependency's
//! include dir is what `#include <stdint.h>` or `#include <version>`
//! opens - in the consumer's own sources and inside the standard
//! library's headers alike.

use std::collections::{BTreeSet, HashMap};

use cabin_workspace::{PackageGraph, PackageKind};
use camino::Utf8Path;

use super::{TargetDepEdge, TargetId, find_target, promote_dir};
use crate::error::BuildError;

/// Every C (through C23) and C++ (through C++26, including the
/// `<cxxx>` forms and the removed `<ccomplex>`-style headers that
/// implementations still ship) standard library header name.
const STANDARD_HEADERS: &[&str] = &[
    "algorithm",
    "any",
    "array",
    "assert.h",
    "atomic",
    "barrier",
    "bit",
    "bitset",
    "cassert",
    "ccomplex",
    "cctype",
    "cerrno",
    "cfenv",
    "cfloat",
    "charconv",
    "chrono",
    "cinttypes",
    "ciso646",
    "climits",
    "clocale",
    "cmath",
    "codecvt",
    "compare",
    "complex",
    "complex.h",
    "concepts",
    "condition_variable",
    "contracts",
    "coroutine",
    "csetjmp",
    "csignal",
    "cstdalign",
    "cstdarg",
    "cstdbool",
    "cstddef",
    "cstdint",
    "cstdio",
    "cstdlib",
    "cstring",
    "ctgmath",
    "ctime",
    "ctype.h",
    "cuchar",
    "cwchar",
    "cwctype",
    "debugging",
    "deque",
    "errno.h",
    "exception",
    "execution",
    "expected",
    "fenv.h",
    "filesystem",
    "flat_map",
    "flat_set",
    "float.h",
    "format",
    "forward_list",
    "fstream",
    "functional",
    "future",
    "generator",
    "hazard_pointer",
    "hive",
    "initializer_list",
    "inplace_vector",
    "inttypes.h",
    "iomanip",
    "ios",
    "iosfwd",
    "iostream",
    "iso646.h",
    "istream",
    "iterator",
    "latch",
    "limits",
    "limits.h",
    "linalg",
    "list",
    "locale",
    "locale.h",
    "map",
    "math.h",
    "mdspan",
    "memory",
    "memory_resource",
    "meta",
    "mutex",
    "new",
    "numbers",
    "numeric",
    "optional",
    "ostream",
    "print",
    "queue",
    "random",
    "ranges",
    "ratio",
    "rcu",
    "regex",
    "scoped_allocator",
    "semaphore",
    "set",
    "setjmp.h",
    "shared_mutex",
    "signal.h",
    "simd",
    "source_location",
    "span",
    "spanstream",
    "sstream",
    "stack",
    "stacktrace",
    "stdalign.h",
    "stdarg.h",
    "stdatomic.h",
    "stdbit.h",
    "stdbool.h",
    "stdckdint.h",
    "stddef.h",
    "stdexcept",
    "stdfloat",
    "stdint.h",
    "stdio.h",
    "stdlib.h",
    "stdnoreturn.h",
    "stop_token",
    "streambuf",
    "string",
    "string.h",
    "string_view",
    "strstream",
    "syncstream",
    "system_error",
    "text_encoding",
    "tgmath.h",
    "thread",
    "threads.h",
    "time.h",
    "tuple",
    "type_traits",
    "typeindex",
    "typeinfo",
    "uchar.h",
    "unordered_map",
    "unordered_set",
    "utility",
    "valarray",
    "variant",
    "vector",
    "version",
    "wchar.h",
    "wctype.h",
];

/// The standard header an include dir entry named `name` would stand
/// in for on a case-insensitive file system, judged the same on every
/// host: `VERSION` for `<version>`.  Lowercasing, uppercasing, then
/// lowercasing again also maps the non-ASCII spellings APFS folds
/// onto ASCII (`ſtdio.h`, `ﬆdio.h`, `caßert`).
fn standard_header_for(name: &str) -> Option<&'static str> {
    let folded = name.to_lowercase().to_uppercase().to_lowercase();
    STANDARD_HEADERS
        .iter()
        .copied()
        .find(|header| *header == folded)
}

/// The standard headers whose names resolve to a file in `dir` on this
/// host.  Opening the name lets the host file system apply exactly the
/// aliasing a compiler's open gets - letter case, Unicode folding and
/// normalization, and code points HFS+ ignores such as U+200C - which
/// [`standard_header_for`] cannot reproduce.
fn probed_standard_headers(dir: &Utf8Path) -> impl Iterator<Item = &'static str> + '_ {
    STANDARD_HEADERS
        .iter()
        .copied()
        .filter(|header| dir.join(header).metadata().is_ok_and(|meta| meta.is_file()))
}

/// Refuse the first registry library target whose include dir holds
/// a file named like a standard header, unless the root manifest's
/// `[standard-header-shadowing] allow` names its package.  Only
/// targets some resolved edge depends on are checked: those are the
/// ones whose include dirs reach another target's compiles.  Local
/// packages are the user's own code and are not checked.  The listed
/// entries catch letter-case variants on every host; the probe catches
/// whatever else this host's file system resolves to a standard name.
pub(super) fn reject_standard_header_shadowing(
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    graph: &PackageGraph,
) -> Result<(), BuildError> {
    let allowed = &graph.root_settings.allow_standard_header_shadowing;
    let deps: BTreeSet<&TargetId> = resolved.values().flatten().map(|edge| &edge.to).collect();
    for (pkg_idx, target_name) in deps {
        let pkg = &graph.packages[*pkg_idx];
        if pkg.kind != PackageKind::Registry || allowed.contains(&pkg.package.name) {
            continue;
        }
        let Some(target) = find_target(&pkg.package, target_name) else {
            continue;
        };
        if !target.kind.is_library_like() {
            continue;
        }
        let manifest_dir = promote_dir(&pkg.manifest_dir)?;
        for include_dir in &target.include_dirs {
            let dir = manifest_dir.join(include_dir);
            let listed = dir
                .read_dir_utf8()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let header = standard_header_for(entry.file_name())?;
                    // Compilers skip a directory named like the
                    // header but follow a symlink to a file.
                    (!entry.path().is_dir()).then(|| (entry.file_name().to_owned(), header))
                });
            let probed = probed_standard_headers(&dir).map(|header| (header.to_owned(), header));
            if let Some((file, header)) = listed.chain(probed).min() {
                return Err(BuildError::StandardHeaderShadowing {
                    package: pkg.package.name.as_str().to_owned(),
                    target: target.name.as_str().to_owned(),
                    include_dir: include_dir.clone(),
                    case_folded: file != header,
                    path: dir.join(file),
                    header,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_case_folded_spellings_of_standard_headers() {
        for (name, header) in [
            ("stdint.h", "stdint.h"),
            ("VERSION", "version"),
            ("Vector", "vector"),
            ("cstdio", "cstdio"),
            ("stdckdint.h", "stdckdint.h"),
            ("\u{17f}tdio.h", "stdio.h"),
            ("\u{fb06}dio.h", "stdio.h"),
            ("stac\u{212a}", "stack"),
            ("ca\u{df}ert", "cassert"),
            ("\u{131}ostream", "iostream"),
        ] {
            assert_eq!(standard_header_for(name), Some(header), "{name}");
        }
    }

    #[test]
    fn ignores_non_standard_names() {
        for name in [
            "sqlite3.h",
            "version.h",
            "VERSION.txt",
            "README",
            "stdio",
            "vector.hpp",
            "unistd.h",
            "__config",
        ] {
            assert_eq!(standard_header_for(name), None, "{name}");
        }
    }

    #[test]
    fn probe_finds_a_standard_header_file_but_not_a_directory() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        std::fs::create_dir(dir.join("vector")).unwrap();
        std::fs::write(dir.join("stdint.h"), "").unwrap();
        std::fs::write(dir.join("sqlite3.h"), "").unwrap();
        assert_eq!(
            probed_standard_headers(dir).collect::<Vec<_>>(),
            ["stdint.h"]
        );
    }
}
