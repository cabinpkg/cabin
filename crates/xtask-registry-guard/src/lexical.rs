//! The shared lexical half of the source guards: blanking comment and
//! string bodies so the guards' scans can match across lines without a
//! comment hiding a call, faking one, or swallowing the code after a
//! URL.  One copy on purpose - the blanker is what makes the guards
//! evasion-resistant, and two drifting copies would be an evasion
//! vector of their own - which is why both guards live in this crate
//! rather than growing a second blanker each.

/// The source with every comment and string body blanked, newlines kept
/// (so reported line numbers still point at the source).  Rust's block
/// comments nest, and a `//` or `/*` inside a string starts nothing - so
/// this walks the file rather than running a pattern over it.
///
/// Bytes in, bytes out: the scan never decodes, so an invalid UTF-8
/// sequence in a source file cannot make the guard skip a file it would
/// otherwise inspect.
#[must_use]
pub fn blank_comments_and_strings(src: &[u8]) -> Vec<u8> {
    let n = src.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let two = &src[i..n.min(i + 2)];
        if two == b"//" {
            let end = src[i..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(n, |at| i + at);
            out.resize(out.len() + (end - i), b' ');
            i = end;
        } else if two == b"/*" {
            let start = i;
            let mut depth = 0i32;
            while i < n {
                let here = &src[i..n.min(i + 2)];
                if here == b"/*" {
                    depth += 1;
                    i += 2;
                } else if here == b"*/" {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            // An unterminated block comment blanks to end of file.
            blank_into(&mut out, &src[start..i]);
        } else if let Some(close) = raw_string_end(src, i) {
            blank_into(&mut out, &src[i..close]);
            i = close;
        } else if src[i] == b'"' {
            let mut j = i + 1;
            while j < n {
                let c = src[j];
                if c == b'"' {
                    break;
                }
                j += if c == b'\\' { 2 } else { 1 };
            }
            // An unterminated string - or one whose last escape stepped
            // past the end - blanks to end of file.
            let j = if j < n { j + 1 } else { n };
            blank_into(&mut out, &src[i..j]);
            i = j;
        } else if let Some(len) = char_literal_len(src, i) {
            // A character literal - `'"'` must not open a string.  A
            // lifetime (`&'a str`) has no closing quote and falls
            // through to the ordinary branch below.
            out.resize(out.len() + len, b' ');
            i += len;
        } else {
            out.push(src[i]);
            i += 1;
        }
    }
    out
}

/// Appends `span` with every byte but its newlines replaced by a space.
fn blank_into(out: &mut Vec<u8>, span: &[u8]) {
    out.extend(span.iter().map(|&b| if b == b'\n' { b'\n' } else { b' ' }));
}

/// One past the end of the raw string literal opening at `i`, or `None`
/// when nothing opens there.  A raw string ends at the quote followed by
/// as many hashes as it opened with; an unterminated one runs to the end
/// of the file.
fn raw_string_end(src: &[u8], i: usize) -> Option<usize> {
    let n = src.len();
    if src[i] != b'r' {
        return None;
    }
    let mut quote = i + 1;
    while quote < n && src[quote] == b'#' {
        quote += 1;
    }
    if quote >= n || src[quote] != b'"' {
        return None;
    }
    let hashes = quote - (i + 1);
    let mut at = quote + 1;
    while at + hashes < n {
        if src[at] == b'"' && src[at + 1..at + 1 + hashes].iter().all(|&b| b == b'#') {
            return Some(at + 1 + hashes);
        }
        at += 1;
    }
    Some(n)
}

/// The length of the character (or byte-character) literal starting at
/// `i`, or `None` when nothing starts there.
fn char_literal_len(src: &[u8], i: usize) -> Option<usize> {
    let n = src.len();
    let mut j = if src[i] == b'b' { i + 1 } else { i };
    if j >= n || src[j] != b'\'' {
        return None;
    }
    j += 1;
    match src.get(j)? {
        // An escape takes the next byte with it, but never a newline:
        // an unterminated `'\` is not a literal.
        b'\\' => {
            if *src.get(j + 1)? == b'\n' {
                return None;
            }
            j += 2;
        }
        b'\'' | b'\n' => return None,
        _ => j += 1,
    }
    if *src.get(j)? != b'\'' {
        return None;
    }
    Some(j + 1 - i)
}
