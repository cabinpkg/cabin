// node:test suite for the strict-zip-profile parser (`npm test`). The
// fixtures are profile archives built by hand right here - local records,
// central directory, EOCD, in the profile's exact tiling - so the parser
// is exercised against the same byte layout `cabin package` produces.
import assert from "node:assert/strict";
import { test } from "node:test";
import { deflateRawSync } from "node:zlib";
import {
    buildTree,
    decodeText,
    parseCentralDirectory,
    parseContentRange,
    parseEocd,
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
