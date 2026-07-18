# The Strict Zip Archive Profile

The canonical package archive is a **zip container** conforming to a single
strict profile. This page is the normative spec: exactly what
`cabin package` writes, what a publish accepts, and what
`cabin-registry-verify` enforces. There is one producer (`cabin-package`)
and one consumer (the verifier), so the profile is deliberately narrow -
Go-module-zip strictness - and everything outside it is rejected rather than
tolerated.

Zip is the format because the registry stores archives as opaque,
content-addressed bytes and a planned source-code viewer needs **random
access** into them; a tar.gz would force either a server-side repack or a
second derived artifact, both infeasible against the Workers CPU and R2
budgets (see [`architecture.md`](architecture.md), "Why a strict zip
profile"). The index document keeps its declared `source.format` as `"zip"`
([`../../docs/package-index.md`](../../docs/package-index.md)).

The profile makes container inspection cheap: the end-of-central-directory
record (EOCD) sits at a fixed offset, records tile the file contiguously,
and every field a hostile archive could hide behind (data descriptors, extra
fields, zip64, comments, local/central disagreement) is banned. The Worker's
publish path can therefore reject a non-zip with O(1) fixed-offset reads, and
the verifier hand-parses the container without the ambiguities a
general-purpose zip library papers over.

## Container

The file is exactly three regions, in order and adjacent:

```
[local file records] [central directory] [EOCD (22 bytes)]
```

- The file begins with the local file header signature `PK\x03\x04` and is at
  least 22 bytes (a bare EOCD).
- The EOCD signature `PK\x05\x06` sits at exactly `len - 22`: the EOCD comment
  length is `0`, so the record is the file's last 22 bytes and is found by a
  fixed-offset read, never a backward scan.
- The EOCD's disk-number and starting-disk fields are `0` (single disk), and
  its "entries on this disk" count equals its "total entries" count.
- No zip64: neither the zip64 EOCD record nor its locator may appear, and no
  count or offset field may hold the `0xFFFF` / `0xFFFFFFFF` zip64 sentinel.
- `cd_offset + cd_size + 22 == len` (the central directory abuts the EOCD, and
  the local records abut the central directory - see [Layout](#layout)).

## Entry names

Entry names are the archive's only path surface. A name is UTF-8 bytes with
`/` separators, stored verbatim (raw bytes) in the local and central headers.
Names are compared as **code points**: Unicode normalization (NFC/NFD)
aliasing is deliberately **not** checked - two names that differ only by
normalization form (a legacy HFS+ hazard) are treated as distinct, out of
profile scope.

A name is rejected when it:

- is absolute (a leading `/`, or a Windows drive/UNC form) - `absolute_path`;
- contains a `..` component - `path_traversal`;
- is empty, is not valid UTF-8, contains a `\`, or has an empty or `.`
  component - `invalid_path`;
- ends in `/` (a directory marker; this profile carries files only) -
  `invalid_path`;
- exceeds the path-length cap (see [Caps](#caps)) - `path_too_long`.

The remaining name rules are the **portability set**, enforced identically at
pack time (`cabin-package`) and at verify time through one shared predicate in
`cabin-fs` so the two sides cannot drift. A name is `invalid_path` when any
component:

- contains a `:` (drive or NTFS alternate-data-stream separator);
- contains a control character (`U+0000`-`U+001F`, NUL included) or one of the
  Win32-forbidden characters `< > " | ? *`;
- ends in a `.` or a space, or begins with a space (Windows silently strips
  trailing dots and spaces, and a leading space aliases the same way);
- is a reserved Windows device stem - `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`,
  `LPT1`-`LPT9`, and the superscript `COM¹` / `LPT²` forms - with or without an
  extension.

`\` is already covered above; it is listed there because it is the one
portability rule that also changes traversal semantics.

### Collisions

- Two entries with byte-identical names - `duplicate_path`.
- Two names that fold to the same string under **Unicode default lowercasing**
  (the locale-independent full lowercasing the Unicode Character Database
  defines via `toLowercase`; not any single language's casing) collide on a
  case-insensitive filesystem - `case_conflict`.
- A regular file used as another entry's parent directory (`src` alongside
  `src/main.cc`): no extractor can materialize both. Exact-name form -
  `path_conflict`; case-folded form (`a` vs `A/b`) - `case_conflict`.

## Entry types

The profile carries **regular files only**; directories are implied by the
paths of the files under them and are never stored as entries.

The type gate reads `S_IFMT` from the external attributes independently of the
version-made-by "made by" system byte, so it does not depend on the producer's
platform:

- any present non-regular type (`S_IFLNK`, `S_IFDIR`, `S_IFCHR`, ...) -
  `forbidden_entry_type`;
- the DOS directory attribute (`0x10`) set - `forbidden_entry_type`;
- a name ending in `/` - `invalid_path` (a directory marker, caught as a name
  rule above).

An absent or zero mode is a regular file. Permission bits are ignored (the
extractor does not honor stored modes; see [Determinism](#determinism)).

## Encoding

- **Methods**: each entry is stored (`0`) or deflated (`8`); no other
  compression method - `unsupported_zip_feature`.
- **General-purpose flags**: every GP bit is `0` except bit 11 (UTF-8/EFS),
  which MUST be set **iff** the name contains non-ASCII bytes and MUST be clear
  otherwise. A non-ASCII name with bit 11 clear would be CP437-decoded by
  common readers, so the bit is required for such names and forbidden for
  ASCII ones. Any other GP bit set - `unsupported_zip_feature`.
- **No data descriptors**: GP bit 3 is `0`, so each entry's compressed size,
  uncompressed size, and CRC-32 live in its local header, not in a trailing
  descriptor - `unsupported_zip_feature` on bit 3.
- **No extra fields**: the extra-field length is `0` in both the local and the
  central header. This structurally forecloses the zip64 extra field (`0x0001`)
  and the Unix path-override extra field (`0x7075`) - `unsupported_zip_feature`.
- **No comments**: no per-entry comment (central comment length `0`), and the
  EOCD comment length is `0`.
- **Decompression integrity**: a stored entry has `csize == usize`; a deflated
  entry must decompress to a clean stream end that consumes exactly its `csize`
  compressed bytes and yields exactly its `usize` uncompressed bytes (an early
  stream end, truncation, or trailing garbage inside the compressed span is
  `header_mismatch`), and its declared CRC-32 must equal the CRC of the bytes
  produced.

## Layout

The three regions tile the file with no gaps, overlaps, or unlisted bytes.
With no extra fields and no data descriptors, each local record occupies
exactly `30 + name_len + csize` bytes (a 30-byte local file header, the name,
the compressed data). Checkable form:

- the EOCD is at `len - 22` with comment length `0`;
- `cd_offset + cd_size + 22 == len`;
- walking the local records in **central-directory order**, the first starts at
  offset `0`, each `next == this + 30 + name_len + csize`, and the last ends at
  `cd_offset`.

Anything else - a record the central directory does not list, a gap or overlap
between records, bytes prepended before the first record or appended after the
EOCD - fails the tiling and is `archive_invalid`.

An empty archive is impossible: a publishable package's root `cabin.toml` is
required, so a zero-entry container is `manifest_missing`, not a distinct
empty-archive case.

## Determinism

Idempotent re-publish is already guaranteed by content-addressing: identical
source bytes hash to the same checksum and the registry answers `200 no_op`.
The determinism rules therefore split by who enforces them. Pinning cosmetic
producer bytes on the **verifier** would let a future `zip` or deflate-backend bump
invalidate otherwise-valid archives, so the verifier enforces only what changes
what an extractor materializes or how the container parses; the producer pins
the rest and the client's own tests hold it.

**Verifier-enforced** (a violation is a verdict):

- method ∈ {store, deflate};
- GP bits per [Encoding](#encoding) (bit 11 iff non-ASCII, all others `0`);
- extra-field length `0`, no data descriptors, no comments, no zip64;
- contiguous [Layout](#layout);
- local header == central header on the raw name bytes, sizes, CRC-32, method,
  and GP bits;
- declared uncompressed size and CRC-32 == the actual decompressed bytes;
- store entries have `csize == usize`;
- all [entry-name](#entry-names) rules and the [entry-type](#entry-types) bits;
- the [caps](#caps).

**Client-only** (the profile requires them, `cabin-package` pins them, and the
client's tests pin them, but the verifier does not check them):

- the last-modified timestamp is `1980-01-01 00:00:00` in both DOS date/time
  fields (`DateTime::default()`, pinned so a `time`-feature unification cannot
  drift it);
- entries are emitted sorted by path;
- fixed version-made-by / version-needed (`System::Unix`, so a Windows build is
  byte-identical to a Unix one - the writer otherwise defaults to DOS);
- external-attribute **permission** bits for a regular file (`0644`);
- deflate is used at a pinned compression level (`6`).

Only permission bits are client-only; the external-attribute **type** bits stay
verifier-enforced. A stored mode is otherwise cosmetic: the extractor ignores
it.

## Caps

Inspection is bounded by four knobs, each an environment variable read by the
verifier (mechanism is contract, values are configuration):

| Variable | Bounds | Default |
| --- | --- | --- |
| `VERIFY_RATIO_CAP` | decompressed bytes allowed per compressed byte | `10` |
| `VERIFY_ABS_CAP_BYTES` | absolute decompressed-total cap | `256 MiB` |
| `VERIFY_MAX_ENTRIES` | entry count | `10000` |
| `VERIFY_MAX_PATH_LEN` | per-entry path length in bytes | `256` |

The decompressed-total cap for one archive is
`min(max(ratio_cap x compressed_size, floor), abs_cap_bytes)`, where the floor
(`4 MiB` base plus `2048` bytes per permitted entry) covers the container
framing the entry cap permits, so a legitimate archive of many tiny files does
not trip the ratio cap before the entry cap.

Size discipline runs in two places:

- the **sum of declared uncompressed sizes** is a cheap pre-check against the
  cap - `decompressed_too_large`;
- the **actual decompressed output** flows through the archive-global capped
  reader - `decompressed_too_large`.

A declared uncompressed size or CRC-32 that disagrees with the actual
decompressed bytes is `header_mismatch`.

## Reason codes

A rejection records a machine-readable reason. The reason is a `code`,
optionally followed by one parenthesized fixed detail that narrows the cause:
`unsupported_zip_feature (zip64)`, `header_mismatch (crc)`,
`invalid_path (trailing dot)`. The machine code is always the first token; the
detail is fixed text and never echoes archive bytes. The shared `cabin-fs`
predicate returns the violated rule, so the verifier's detail and the pack-time
diagnostic name the same rule.

The container profile defines these codes; three are new to the zip profile
(`case_conflict`, `unsupported_zip_feature`, `header_mismatch`), the rest are
carried over from the previous tar profile:

| Code | Cause |
| --- | --- |
| `archive_invalid` | not a well-formed container: bad EOCD, non-contiguous layout, or bytes outside the tiled regions |
| `unsupported_zip_feature` | a banned feature: method other than store/deflate, a set GP bit other than 11, a nonzero extra field, a comment, zip64, or a data descriptor |
| `header_mismatch` | a local header disagrees with its central header, a stored entry's `csize != usize`, a deflated entry does not cleanly consume its compressed span, or a declared size/CRC disagrees with the decompressed bytes |
| `decompressed_too_large` | the decompression cap was crossed |
| `too_many_entries` | the entry-count cap was crossed |
| `path_too_long` | the path-length cap was crossed |
| `forbidden_entry_type` | a non-regular entry type (symlink, directory attribute, device, ...) |
| `absolute_path` | an absolute entry name |
| `path_traversal` | a `..` component |
| `invalid_path` | a name that is empty, non-UTF-8, `\`-bearing, has an empty or `.` component, is a directory marker, or violates the portability set |
| `duplicate_path` | the same name (raw bytes) twice |
| `case_conflict` | two names that fold to the same string under Unicode default lowercasing, including the file-as-parent-directory form |
| `path_conflict` | a regular file used as another entry's parent directory (exact-name form) |
| `manifest_missing` | no `cabin.toml` at the archive root |

Once the container passes, the manifest-consistency pass runs and can add the
consistency codes (`manifest_invalid`, `name_mismatch`, `checksum_mismatch`,
...); those are unchanged by this profile and are tabulated with the full
public set in
[`../../docs/remote-registry.md`](../../docs/remote-registry.md), "The
verifier's checks".
