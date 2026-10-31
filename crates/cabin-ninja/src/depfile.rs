//! Reading the Makefile depfiles GCC and Clang write for `-MD`, for the
//! compiles whose depfile Ninja's own parser would misread.

use crate::writer::DEPFILE_DEPS_PREFIX;

/// The longest dependency path Ninja's `deps = msvc` accepts on Windows,
/// where it fails the edge on a longer one.
const MAX_MSVC_DEPS_PATH: usize = 260;

/// What `cabin depfile` prints for Ninja's `deps = msvc`: a line of
/// [`DEPFILE_DEPS_PREFIX`] and the path for each prerequisite of the
/// depfile `content`.
///
/// Ninja drops a dependency whose path contains `program files` or
/// `microsoft visual studio` as a system header, and on Windows rejects
/// one longer than 260 bytes.  Such a path is left out and the report
/// names a file that never exists instead, so the compile reruns on
/// every build.
#[must_use]
pub fn msvc_deps_report(content: &[u8]) -> Vec<u8> {
    let mut report = Vec::new();
    let mut line = |path: &[u8]| {
        report.extend_from_slice(DEPFILE_DEPS_PREFIX.as_bytes());
        report.push(b' ');
        report.extend_from_slice(path);
        report.push(b'\n');
    };
    let mut dropped = false;
    for path in depfile_prerequisites(content) {
        let lower = path.to_ascii_lowercase();
        if (cfg!(windows) && path.len() > MAX_MSVC_DEPS_PATH)
            || [&b"program files"[..], b"microsoft visual studio"]
                .iter()
                .any(|name| lower.windows(name.len()).any(|window| window == *name))
        {
            dropped = true;
        } else {
            line(&path);
        }
    }
    // Relative, so Ninja looks for it in the build root, where Cabin never
    // creates it, and the build root's own path, which may hold one of
    // those names, cannot get it dropped too.
    if dropped {
        line(b"cabin-untracked-dependency");
    }
    report
}

/// The prerequisites a GCC/Clang `-MD` depfile names, in order.
///
/// Undoes the compilers' escaping: a space or tab preceded by 2N+1
/// backslashes is N backslashes and that character, `\#` and `\:` are
/// `#` and `:` (unless the `:` ends a target), and `$$` is `$`.  An
/// unescaped space, carriage return, or newline and a backslash before a
/// newline separate words, an unescaped newline ends a rule, and the
/// words of a rule up to one ending in `:` are its targets.  Unlike
/// Ninja, which ends a path at `;`, `|`, a tab, and similar bytes and
/// keeps the backslash of an escaped tab, every other byte is part of
/// the path: GCC escapes a tab and Clang writes it as is, and neither
/// separates words with one.
fn depfile_prerequisites(content: &[u8]) -> Vec<Vec<u8>> {
    let mut prerequisites = Vec::new();
    let mut rule: Vec<Vec<u8>> = Vec::new();
    let mut word = Vec::new();
    let mut i = 0;
    while i < content.len() {
        let byte = content[i];
        i += 1;
        match byte {
            b'\\' => {
                let run = 1 + content[i..].iter().take_while(|&&b| b == b'\\').count();
                i += run - 1;
                let (kept, escaped) = match content.get(i) {
                    Some(&c @ (b' ' | b'\t')) if run % 2 == 1 => (run / 2, Some(c)),
                    Some(b'#') => (run - 1, Some(b'#')),
                    Some(b':')
                        if !matches!(
                            content.get(i + 1),
                            None | Some(0 | b' ' | b'\t' | b'\r' | b'\n')
                        ) =>
                    {
                        (run - 1, Some(b':'))
                    }
                    Some(b'\n') => (run - 1, None),
                    Some(b'\r') if content.get(i + 1) == Some(&b'\n') => {
                        i += 1;
                        (run - 1, None)
                    }
                    _ => (run, None),
                };
                word.extend(std::iter::repeat_n(b'\\', kept));
                match escaped {
                    Some(c) => {
                        word.push(c);
                        i += 1;
                    }
                    // A line continuation separates words.
                    None if kept < run => {
                        i += 1;
                        rule.extend((!word.is_empty()).then(|| std::mem::take(&mut word)));
                    }
                    None => {}
                }
            }
            b'$' if content.get(i) == Some(&b'$') => {
                word.push(b'$');
                i += 1;
            }
            b' ' | b'\r' | b'\n' => {
                rule.extend((!word.is_empty()).then(|| std::mem::take(&mut word)));
                if byte == b'\n' {
                    end_rule(&mut rule, &mut prerequisites);
                }
            }
            _ => word.push(byte),
        }
    }
    rule.extend((!word.is_empty()).then_some(word));
    end_rule(&mut rule, &mut prerequisites);
    prerequisites
}

/// Move the words of `rule` after its targets into `prerequisites`.
fn end_rule(rule: &mut Vec<Vec<u8>>, prerequisites: &mut Vec<Vec<u8>>) {
    if let Some(colon) = rule.iter().position(|word| word.ends_with(b":")) {
        prerequisites.extend(rule.drain(colon + 1..));
    }
    rule.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Vec<String> {
        depfile_prerequisites(content.as_bytes())
            .into_iter()
            .map(|p| String::from_utf8(p).unwrap())
            .collect()
    }

    #[test]
    fn keeps_the_bytes_ninja_would_split_on() {
        // GCC 16 output for headers whose directory names carry the bytes
        // Ninja's depfile lexer ends a path at; GCC escapes none of them.
        let gcc = "/b/main.o: /p/main.c \\\n /p/semi;c/h.h \\\n /p/apo's/h.h \\\n \
                   /p/dq\"x/h.h \\\n /p/pi|pe/h.h \\\n /p/amp&x/h.h /p/q?x/h.h \\\n \
                   /p/st*x/h.h /p/lt<x/h.h /p/gt>x/h.h /p/car^x/h.h /p/bt`x/h.h\n";
        assert_eq!(
            parse(gcc),
            [
                "/p/main.c",
                "/p/semi;c/h.h",
                "/p/apo's/h.h",
                "/p/dq\"x/h.h",
                "/p/pi|pe/h.h",
                "/p/amp&x/h.h",
                "/p/q?x/h.h",
                "/p/st*x/h.h",
                "/p/lt<x/h.h",
                "/p/gt>x/h.h",
                "/p/car^x/h.h",
                "/p/bt`x/h.h",
            ]
        );
    }

    #[test]
    fn undoes_gcc_and_clang_escapes() {
        // Spellings GCC 16 and Clang 23 write for a space, a tab, `#`, `$`,
        // and backslashes before and away from a space.
        let depfile = "/b/x.o: /p/sp\\ ace/h.h /p/tab\\\tx/h.h /p/ha\\#sh/h.h \
                       /p/do$$lr/h.h /p/bs\\x/h.h /p/bs\\\\\\ sp/h.h /p/co:lon/h.h \
                       C:\\src\\a.c\n";
        assert_eq!(
            parse(depfile),
            [
                "/p/sp ace/h.h",
                "/p/tab\tx/h.h",
                "/p/ha#sh/h.h",
                "/p/do$lr/h.h",
                "/p/bs\\x/h.h",
                "/p/bs\\ sp/h.h",
                "/p/co:lon/h.h",
                "C:\\src\\a.c",
            ]
        );
    }

    #[test]
    fn reads_a_raw_tab_as_part_of_a_path() {
        // Clang 23 writes a tab in a path as is.
        let depfile = "/b/x.o: /p/ta\tb/x.c \\\n  /p/ta\tb/h.h\n";
        assert_eq!(parse(depfile), ["/p/ta\tb/x.c", "/p/ta\tb/h.h"]);
    }

    #[test]
    fn reads_escaped_colons_and_keeps_other_control_bytes() {
        // ccache escapes the colons of the depfiles it rewrites, as Ninja
        // reads them; a form feed is part of a name, not a separator.
        let depfile = "/b/x.o: D\\:/sdk/api.h /p/ff\x0cdir/h.h\n";
        assert_eq!(parse(depfile), ["D:/sdk/api.h", "/p/ff\x0cdir/h.h"]);
    }

    #[test]
    fn skips_every_target_and_reads_continued_lines() {
        let depfile = "/b/x.o /b/y.o: \\\r\n  /p/a.h \\\n  /p/b.h\n/p/a.h:\n";
        assert_eq!(parse(depfile), ["/p/a.h", "/p/b.h"]);
        assert!(parse("").is_empty());
    }

    #[test]
    fn reports_a_missing_path_when_ninja_would_drop_a_dependency() {
        let report =
            |content: &str| String::from_utf8(msvc_deps_report(content.as_bytes())).unwrap();
        assert_eq!(
            report("/b/x.o: /p/a;b/x.c /p/a;b/x.h\n"),
            "cabin-depfile: /p/a;b/x.c\ncabin-depfile: /p/a;b/x.h\n"
        );
        assert_eq!(
            report("/b/x.o: /p/a;b/x.c C:/Program\\ Files/sdk/api.h\n"),
            "cabin-depfile: /p/a;b/x.c\ncabin-depfile: cabin-untracked-dependency\n"
        );
        let long = format!("/b/x.o: /p/a;b/x.c /p/{}.h\n", "d".repeat(300));
        assert_eq!(
            report(&long),
            if cfg!(windows) {
                "cabin-depfile: /p/a;b/x.c\ncabin-depfile: cabin-untracked-dependency\n".to_owned()
            } else {
                format!(
                    "cabin-depfile: /p/a;b/x.c\ncabin-depfile: /p/{}.h\n",
                    "d".repeat(300)
                )
            }
        );
    }

    #[test]
    fn arbitrary_bytes_read_into_no_more_bytes() {
        // Seeded xorshift, so a failure reproduces.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        // Weighted toward the bytes the reader branches on.
        let special = b"\\ \t\r\n:#$";
        for _ in 0..2000 {
            let len = usize::try_from(next() % 512).unwrap();
            let content: Vec<u8> = (0..len)
                .map(|_| match next() % 4 {
                    0 => special[usize::from(next().to_le_bytes()[0]) % special.len()],
                    _ => next().to_le_bytes()[0],
                })
                .collect();
            let read: usize = depfile_prerequisites(&content).iter().map(Vec::len).sum();
            assert!(read <= content.len(), "{content:?}");
        }
    }
}
