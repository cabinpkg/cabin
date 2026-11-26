// node:test suite for the strict-zip-profile parser (`npm test`). The
// fixtures are profile archives built by hand right here - local records,
// central directory, EOCD, in the profile's exact tiling - so the parser
// is exercised against the same byte layout `cabin package` produces.
import assert from "node:assert/strict";
import { test } from "node:test";
import { deflateRawSync, crc32 as zlibCrc32 } from "node:zlib";
import {
    buildTree,
    crc32,
    decodeText,
    inflatePrefix,
    isRequestedSpan,
    MAX_DISPLAY_BYTES,
    matchesEntry,
    parseCentralDirectory,
    parseContentRange,
    parseEocd,
    readExactly,
} from "./sourceArchive.ts";

interface FixtureFile {
    name: string;
    data: Uint8Array;
    method: 0 | 8;
}

function fixtureFile(name: string, text: string, method: 0 | 8): FixtureFile {
    return { name, data: new TextEncoder().encode(text), method };
}

// Builds a profile archive: contiguous local records, then the central
// directory, then the 22-byte EOCD. Sizes, CRCs, and offsets are real;
// fields the parser ignores (times, versions) stay zero.
function buildArchive(files: FixtureFile[]): Uint8Array {
    const encoder = new TextEncoder();
    const locals: Uint8Array[] = [];
    const centrals: Uint8Array[] = [];
    let offset = 0;
    for (const file of files) {
        const name = encoder.encode(file.name);
        const compressed =
            file.method === 8 ? deflateRawSync(file.data) : file.data;
        const local = new Uint8Array(30 + name.length + compressed.length);
        const lv = new DataView(local.buffer);
        lv.setUint32(0, 0x04034b50, true);
        lv.setUint16(8, file.method, true);
        lv.setUint32(14, zlibCrc32(file.data), true);
        lv.setUint32(18, compressed.length, true);
        lv.setUint32(22, file.data.length, true);
        lv.setUint16(26, name.length, true);
        local.set(name, 30);
        local.set(compressed, 30 + name.length);
        locals.push(local);

        const central = new Uint8Array(46 + name.length);
        const cv = new DataView(central.buffer);
        cv.setUint32(0, 0x02014b50, true);
        cv.setUint16(10, file.method, true);
        cv.setUint32(16, zlibCrc32(file.data), true);
        cv.setUint32(20, compressed.length, true);
        cv.setUint32(24, file.data.length, true);
        cv.setUint16(28, name.length, true);
        cv.setUint32(42, offset, true);
        central.set(name, 46);
        centrals.push(central);
        offset += local.length;
    }
    const cdSize = centrals.reduce((sum, c) => sum + c.length, 0);
    const eocd = new Uint8Array(22);
    const ev = new DataView(eocd.buffer);
    ev.setUint32(0, 0x06054b50, true);
    ev.setUint16(8, files.length, true);
    ev.setUint16(10, files.length, true);
    ev.setUint32(12, cdSize, true);
    ev.setUint32(16, offset, true);
    const archive = new Uint8Array(offset + cdSize + 22);
    let at = 0;
    for (const part of [...locals, ...centrals, eocd]) {
        archive.set(part, at);
        at += part.length;
    }
    return archive;
}

const FILES = [
    fixtureFile("cabin.toml", '[package]\nname = "smoke/withdep"\n', 0),
    fixtureFile("src/main.cc", "int main() { return 0; }\n", 8),
    fixtureFile("src/näme.hh", "#pragma once\n", 8),
];

test("the EOCD and central directory of a profile archive parse", () => {
    const archive = buildArchive(FILES);
    // Any suffix works; the EOCD is the last 22 bytes by profile.
    const layout = parseEocd(
        archive.subarray(archive.length - 60),
        archive.length,
    );
    assert.equal(layout.entryCount, 3);
    assert.equal(layout.cdOffset + layout.cdSize + 22, archive.length);

    const cd = archive.subarray(
        layout.cdOffset,
        layout.cdOffset + layout.cdSize,
    );
    const entries = parseCentralDirectory(cd, layout);
    assert.deepEqual(
        entries.map((entry) => entry.name),
        ["cabin.toml", "src/main.cc", "src/näme.hh"],
    );
    // Each entry carries the checksum of its own content, and the
    // viewer's CRC-32 is the zip one.
    assert.deepEqual(
        entries.map((entry) => entry.crc32),
        FILES.map((file) => crc32(file.data)),
    );
    assert.equal(crc32(new TextEncoder().encode("123456789")), 0xcbf43926);
    assert.equal(crc32(new Uint8Array()), 0);
    const everyByte = Uint8Array.from({ length: 4096 }, (_, at) => at);
    assert.equal(crc32(everyByte), zlibCrc32(everyByte));
    // The data span computed from central metadata alone must slice the
    // exact bytes the builder wrote: store verbatim, deflate inflatable.
    const stored = entries[0];
    assert.equal(stored.method, 0);
    assert.deepEqual(
        archive.subarray(stored.dataStart, stored.dataStart + stored.csize),
        FILES[0].data,
    );
    const deflated = entries[1];
    assert.equal(deflated.method, 8);
    assert.deepEqual(
        new Uint8Array(deflateRawSync(FILES[1].data)),
        archive.subarray(
            deflated.dataStart,
            deflated.dataStart + deflated.csize,
        ),
    );
    // The non-ASCII name's dataStart uses byte lengths, not decoded
    // string lengths.
    const unicode = entries[2];
    assert.deepEqual(
        new Uint8Array(deflateRawSync(FILES[2].data)),
        archive.subarray(unicode.dataStart, unicode.dataStart + unicode.csize),
    );
});

test("malformed containers are refused", () => {
    const archive = buildArchive(FILES);
    // A tail too short to hold an EOCD.
    assert.throws(() =>
        parseEocd(archive.subarray(archive.length - 10), archive.length),
    );
    // A wrong signature (a truncated or non-zip body).
    const noise = new Uint8Array(64);
    assert.throws(() => parseEocd(noise, 64));
    // A size disagreeing with the EOCD's tiling (a corrupt total).
    assert.throws(() => parseEocd(archive, archive.length + 1));
    // A nonzero comment length (out of profile).
    const commented = archive.slice();
    new DataView(commented.buffer).setUint16(commented.length - 2, 1, true);
    assert.throws(() => parseEocd(commented, commented.length));

    const layout = parseEocd(archive, archive.length);
    const cd = archive.slice(layout.cdOffset, layout.cdOffset + layout.cdSize);
    // A short read of the directory itself.
    assert.throws(() => parseCentralDirectory(cd.subarray(1), layout));
    // An entry count disagreeing with the records.
    assert.throws(() =>
        parseCentralDirectory(cd, { ...layout, entryCount: 2 }),
    );
    // A local offset whose data span would run into the directory.
    const overlapping = cd.slice();
    new DataView(overlapping.buffer).setUint32(42, layout.cdOffset, true);
    assert.throws(() => parseCentralDirectory(overlapping, layout));
});

test("the tree nests directories and sorts them itself", () => {
    // Deliberately unsorted: central-directory order is producer-only,
    // so the tree must sort.
    const archive = buildArchive([
        fixtureFile("src/z.cc", "z", 0),
        fixtureFile("cabin.toml", "t", 0),
        fixtureFile("src/a.cc", "a", 0),
        fixtureFile("include/x/y.hh", "y", 0),
    ]);
    const layout = parseEocd(archive, archive.length);
    const entries = parseCentralDirectory(
        archive.subarray(layout.cdOffset, layout.cdOffset + layout.cdSize),
        layout,
    );
    const tree = buildTree(entries);
    assert.deepEqual(
        tree.files.map((file) => file.name),
        ["cabin.toml"],
    );
    assert.deepEqual([...tree.directories.keys()], ["include", "src"]);
    assert.deepEqual(
        tree.directories.get("src")?.files.map((file) => file.name),
        ["src/a.cc", "src/z.cc"],
    );
    assert.deepEqual(
        tree.directories.get("include")?.directories.get("x")?.files[0]?.name,
        "include/x/y.hh",
    );
});

test("content-range headers parse strictly", () => {
    assert.deepEqual(parseContentRange("bytes 78-99/100"), {
        first: 78,
        last: 99,
        size: 100,
    });
    for (const header of [
        null,
        "",
        "bytes */100",
        "bytes 0-99",
        "bytes 99-78/100",
        "bytes 0-100/100",
        "octets 0-1/2",
    ]) {
        assert.equal(parseContentRange(header), null, String(header));
    }
});

test("a response must be the span that was asked for", () => {
    const span = (first: number, last: number, size: number) => ({
        first,
        last,
        size,
    });
    // The suffix request: the last bytes, or a shorter archive whole.
    assert.ok(isRequestedSpan(span(34464, 99999, 100000), null, 65536, null));
    assert.ok(isRequestedSpan(span(0, 99, 100), null, 65536, null));
    assert.ok(!isRequestedSpan(span(0, 65535, 100000), null, 65536, null));
    assert.ok(!isRequestedSpan(span(34464, 99998, 100000), null, 65536, null));
    // An offset request: exactly those bytes, of the archive already seen.
    assert.ok(isRequestedSpan(span(10, 14, 100), 10, 5, 100));
    assert.ok(!isRequestedSpan(span(11, 15, 100), 10, 5, 100));
    assert.ok(!isRequestedSpan(span(10, 13, 100), 10, 5, 100));
    assert.ok(!isRequestedSpan(span(10, 14, 101), 10, 5, 100));
    // The registry accepts no archive over 16 MiB, whatever is asked.
    const cap = 16 * 1024 * 1024;
    assert.ok(isRequestedSpan(span(cap - 22, cap - 1, cap), null, 22, null));
    assert.ok(!isRequestedSpan(span(cap - 21, cap, cap + 1), null, 22, null));
});

test("a body is read to exactly the length asked for, never past it", async () => {
    const body = (chunks: number[][], onCancel = () => {}) =>
        new ReadableStream<Uint8Array>({
            start(controller) {
                for (const chunk of chunks) {
                    controller.enqueue(Uint8Array.from(chunk));
                }
                controller.close();
            },
            cancel: onCancel,
        });
    assert.deepEqual(
        await readExactly(body([[1, 2], [3]]), 3),
        Uint8Array.from([1, 2, 3]),
    );
    assert.equal(await readExactly(body([[1, 2]]), 3), null);
    assert.equal(
        await readExactly(
            body([
                [1, 2],
                [3, 4],
            ]),
            3,
        ),
        null,
    );
    // A body that never ends is dropped at the first byte too many.
    let pulled = 0;
    let cancelled = false;
    const endless = new ReadableStream<Uint8Array>({
        pull(controller) {
            pulled += 1;
            controller.enqueue(new Uint8Array(1024));
        },
        cancel() {
            cancelled = true;
        },
    });
    assert.equal(await readExactly(endless, 4096), null);
    assert.ok(cancelled);
    assert.ok(pulled < 16);
});

test("shown content must be the entry the directory declares", () => {
    const entry = (data: Uint8Array, usize = data.length) => ({
        name: "f",
        method: 8,
        csize: 1,
        usize,
        crc32: crc32(data),
        dataStart: 0,
    });
    const whole = new TextEncoder().encode("int x;\n");
    assert.ok(matchesEntry(entry(whole), whole));
    assert.ok(matchesEntry(entry(new Uint8Array()), new Uint8Array()));
    assert.ok(!matchesEntry({ ...entry(whole), crc32: 1 }, whole));
    // Content shorter than declared is not a truncated view of it.
    assert.ok(!matchesEntry(entry(whole, 2 * MAX_DISPLAY_BYTES), whole));
    // A cut view is a prefix: its entry must be declared past the cap.
    const cut = new Uint8Array(MAX_DISPLAY_BYTES + 1);
    assert.ok(matchesEntry(entry(whole, 2 * MAX_DISPLAY_BYTES), cut));
    assert.ok(!matchesEntry(entry(whole, 4), cut));
});

test("a name keeps a leading U+FEFF, so it cannot pass for another file", () => {
    const archive = buildArchive([
        fixtureFile("cabin.toml", "[package]\n", 0),
        fixtureFile("\uFEFFcabin.toml", "decoy\n", 0),
    ]);
    const layout = parseEocd(
        archive.subarray(archive.length - 22),
        archive.length,
    );
    const cd = archive.subarray(
        layout.cdOffset,
        layout.cdOffset + layout.cdSize,
    );
    assert.deepEqual(
        parseCentralDirectory(cd, layout).map((entry) => entry.name),
        ["cabin.toml", "\uFEFFcabin.toml"],
    );
});

test("inflation returns a whole stream and stops soon after the cap", async () => {
    // Poorly compressible, so its stream spans many input slices.
    let seed = 1;
    const noisy = Uint8Array.from({ length: 8192 }, () => {
        seed = (Math.imul(seed, 1664525) + 1013904223) | 0;
        return seed >>> 24;
    });
    const stream = new Uint8Array(deflateRawSync(noisy));
    assert.ok(stream.length > 4096);
    assert.deepEqual(await inflatePrefix(stream, MAX_DISPLAY_BYTES), noisy);
    const bomb = new Uint8Array(
        deflateRawSync(new Uint8Array(32 * 1024 * 1024)),
    );
    // What bounds the inflater's work per step is the size of each
    // input chunk it is handed, so that is watched at its writable side.
    const Inflater = globalThis.DecompressionStream;
    let largestInput = 0;
    globalThis.DecompressionStream = class extends Inflater {
        override get writable() {
            const writer = super.writable.getWriter();
            return new WritableStream<BufferSource>({
                write(chunk) {
                    largestInput = Math.max(largestInput, chunk.byteLength);
                    return writer.write(chunk);
                },
                close: () => writer.close(),
                abort: (reason) => writer.abort(reason),
            });
        }
    };
    let prefix: Uint8Array;
    try {
        prefix = await inflatePrefix(bomb, MAX_DISPLAY_BYTES);
    } finally {
        globalThis.DecompressionStream = Inflater;
    }
    assert.ok(prefix.length > MAX_DISPLAY_BYTES);
    assert.ok(prefix.length < 4 * MAX_DISPLAY_BYTES);
    assert.ok(largestInput > 0 && largestInput <= 1024);
    await assert.rejects(inflatePrefix(Uint8Array.from([0xff, 0xff]), 16));
});

test("text decoding refuses binaries and tolerates truncated UTF-8", () => {
    const encoder = new TextEncoder();
    assert.equal(decodeText(encoder.encode("plain text"), false), "plain text");
    // A NUL byte marks a binary whatever the rest looks like.
    assert.equal(decodeText(new Uint8Array([104, 105, 0, 106]), false), null);
    // Invalid UTF-8 is binary...
    assert.equal(decodeText(new Uint8Array([0xff, 0xfe]), false), null);
    assert.equal(decodeText(new Uint8Array([0xff, 0xfe]), true), null);
    // ...even when a valid prefix precedes an invalid tail byte, which
    // no truncation cut can produce.
    assert.equal(decodeText(new Uint8Array([0x61, 0xff]), true), null);
    // But a truncation cut mid-sequence is not binary: the incomplete
    // final sequence is held back.
    const cut = encoder.encode("naïve").subarray(0, 3);
    assert.equal(decodeText(cut, false), null);
    assert.equal(decodeText(cut, true), "na");
});
