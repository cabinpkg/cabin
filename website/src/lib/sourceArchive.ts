// Client-side parsing for the registry's strict zip profile
// (registry/docs/archive-format.md in the repository). The source viewer
// range-reads a verified version's archive through the session route and
// parses the container in the browser, so the server stays a byte proxy.
// Only what the viewer needs is parsed: the end-of-central-directory
// record at its profile-pinned tail offset and the central directory.
// Local headers are never fetched - the profile pins them to 30 bytes
// plus the name with no extra field, so each entry's data offset is
// computable from central metadata alone, and only central metadata is
// trusted. Served archives already passed the verifier's full profile
// check, so a violation here is a plain parse error, not something to
// tolerate or work around.
//
// Dependency-free on purpose (like account.ts); the node:test suite in
// sourceArchive.test.ts exercises it against archives built in-test.

const EOCD_SIZE = 22;
const EOCD_SIGNATURE = 0x06054b50;
const CENTRAL_HEADER_SIZE = 46;
const CENTRAL_SIGNATURE = 0x02014b50;
const LOCAL_HEADER_SIZE = 30;

// What one archive slice request may ask for: the server refuses longer
// ranges (registry/src/source.rs MAX_RANGE_BYTES), so larger spans are
// fetched in chunks of this size.
export const MAX_REQUEST_BYTES = 4 * 1024 * 1024;

// The suffix length of the first request: covers the EOCD (the last 22
// bytes; zip64 and comments are banned) and, for typical packages, the
// whole central directory in the same round trip.
export const TAIL_BYTES = 65536;

// Files whose decoded size exceeds this render as a truncated prefix
// with a notice instead of the full content.
export const MAX_DISPLAY_BYTES = 1024 * 1024;

export interface ArchiveLayout {
    /** Total archive size in bytes (from the Content-Range total). */
    size: number;
    cdOffset: number;
    cdSize: number;
    entryCount: number;
}

export interface ArchiveEntry {
    /** The full `/`-separated path, verbatim from the central directory. */
    name: string;
    /** 0 = store, 8 = deflate: the only methods in the profile. */
    method: number;
    /** Compressed size: the length of the span at `dataStart`. */
    csize: number;
    /** Uncompressed size. */
    usize: number;
    /** Archive offset of the entry's first compressed byte. */
    dataStart: number;
}

function view(bytes: Uint8Array): DataView {
    return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
}

// Parses the end-of-central-directory record from the archive's tail
// (any suffix of at least 22 bytes). The profile pins the EOCD to the
// last 22 bytes exactly - comment length zero - so there is no backward
// scan. Throws on anything that is not a profile archive.
export function parseEocd(tail: Uint8Array, size: number): ArchiveLayout {
    if (tail.length < EOCD_SIZE || tail.length > size) {
        throw new Error("the archive tail is too short");
    }
    const v = view(tail);
    const at = tail.length - EOCD_SIZE;
    const commentLength = v.getUint16(at + 20, true);
    if (v.getUint32(at, true) !== EOCD_SIGNATURE || commentLength !== 0) {
        throw new Error("the archive has no end-of-central-directory record");
    }
    const entryCount = v.getUint16(at + 10, true);
    const cdSize = v.getUint32(at + 12, true);
    const cdOffset = v.getUint32(at + 16, true);
    // The profile's tiling: the central directory abuts the EOCD.
    if (cdOffset + cdSize + EOCD_SIZE !== size) {
        throw new Error("the archive layout does not match the profile");
    }
    return { size, cdOffset, cdSize, entryCount };
}

// Walks the central directory (exactly `layout.cdSize` bytes), returning
// one entry per record. Name lengths are byte lengths throughout -
// `dataStart` must never be computed from a decoded string length.
// Throws when the records do not tile the directory exactly or an
// entry's data span would run into the directory itself.
export function parseCentralDirectory(
    cd: Uint8Array,
    layout: ArchiveLayout,
): ArchiveEntry[] {
    if (cd.length !== layout.cdSize) {
        throw new Error("the central directory is incomplete");
    }
    const v = view(cd);
    const decoder = new TextDecoder("utf-8", { fatal: true });
    const entries: ArchiveEntry[] = [];
    let at = 0;
    while (at < cd.length) {
        if (
            at + CENTRAL_HEADER_SIZE > cd.length ||
            v.getUint32(at, true) !== CENTRAL_SIGNATURE
        ) {
            throw new Error("the central directory is malformed");
        }
        const method = v.getUint16(at + 10, true);
        const csize = v.getUint32(at + 20, true);
        const usize = v.getUint32(at + 24, true);
        const nameLength = v.getUint16(at + 28, true);
        const extraLength = v.getUint16(at + 30, true);
        const commentLength = v.getUint16(at + 32, true);
        const localOffset = v.getUint32(at + 42, true);
        const nameEnd = at + CENTRAL_HEADER_SIZE + nameLength;
        if (nameEnd > cd.length) {
            throw new Error("the central directory is malformed");
        }
        // The profile pins local records to a 30-byte header plus the
        // name - no extra field - so the data offset needs no
        // local-header read.
        const dataStart = localOffset + LOCAL_HEADER_SIZE + nameLength;
        if (dataStart + csize > layout.cdOffset) {
            throw new Error("an entry overlaps the central directory");
        }
        entries.push({
            name: decoder.decode(
                cd.subarray(at + CENTRAL_HEADER_SIZE, nameEnd),
            ),
            method,
            csize,
            usize,
            dataStart,
        });
        at = nameEnd + extraLength + commentLength;
    }
    if (at !== cd.length || entries.length !== layout.entryCount) {
        throw new Error("the central directory count does not match");
    }
    return entries;
}

export interface TreeDirectory {
    directories: Map<string, TreeDirectory>;
    files: ArchiveEntry[];
}

// Builds the nested directory tree the viewer renders. Directories are
// implied by file paths (the profile stores no directory entries), and
// everything is sorted here - central-directory order is producer-only
// and not verifier-enforced.
export function buildTree(entries: ArchiveEntry[]): TreeDirectory {
    const root: TreeDirectory = { directories: new Map(), files: [] };
    const sorted = [...entries].sort((a, b) => (a.name < b.name ? -1 : 1));
    for (const entry of sorted) {
        const parts = entry.name.split("/");
        let directory = root;
        for (const part of parts.slice(0, -1)) {
            let next = directory.directories.get(part);
            if (!next) {
                next = { directories: new Map(), files: [] };
                directory.directories.set(part, next);
            }
            directory = next;
        }
        directory.files.push(entry);
    }
    return root;
}

// Parses `Content-Range: bytes <first>-<last>/<size>`, returning the
// span and total. The viewer's first (suffix) request learns the
// archive size from it.
export function parseContentRange(
    header: string | null,
): { first: number; last: number; size: number } | null {
    const match = header?.match(/^bytes (\d+)-(\d+)\/(\d+)$/) ?? null;
    if (!match) {
        return null;
    }
    const [first, last, size] = [
        Number(match[1]),
        Number(match[2]),
        Number(match[3]),
    ];
    if (
        !Number.isSafeInteger(first) ||
        !Number.isSafeInteger(last) ||
        !Number.isSafeInteger(size) ||
        first > last ||
        last >= size
    ) {
        return null;
    }
    return { first, last, size };
}

// Decodes displayable text: UTF-8 with no NUL byte. `truncated` content
// may end mid-sequence, so it decodes in streaming mode, which holds
// back exactly an incomplete final sequence while still throwing on
// genuinely invalid bytes - trimming bytes until a decode succeeds
// would misread binary tails as truncation.
export function decodeText(
    bytes: Uint8Array,
    truncated: boolean,
): string | null {
    if (bytes.includes(0)) {
        return null;
    }
    const decoder = new TextDecoder("utf-8", { fatal: true });
    try {
        return truncated
            ? decoder.decode(bytes, { stream: true })
            : decoder.decode(bytes);
    } catch {
        return null;
    }
}
