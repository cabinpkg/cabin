// Drives /dashboard/source: settles the shared auth probe, then browses
// one published version's archive through the session source route
// (registry/docs/architecture.md, "The source viewer's ranged reads").
// The server is a byte proxy; everything zip happens here - the
// end-of-central-directory tail is range-read first, then the central
// directory, then one file's compressed span at a time, decompressed
// with DecompressionStream. Content renders as text only, always via
// textContent - never markup - with binary and oversized files replaced
// by a notice.
import { bootAccountShell } from "../lib/accountShell";
import { formatBytes } from "../lib/format";
import {
    type ArchiveEntry,
    type ArchiveLayout,
    buildTree,
    decodeText,
    MAX_DISPLAY_BYTES,
    MAX_REQUEST_BYTES,
    parseCentralDirectory,
    parseContentRange,
    parseEocd,
    TAIL_BYTES,
    type TreeDirectory,
} from "../lib/sourceArchive.ts";

// The package/version grammars, mirrored from the registry's route
// validation: everything they admit is URL-path-safe verbatim, so the
// route below is interpolated unencoded (percent-escapes would fail the
// server's charset checks).
const NAME_PATTERN =
    /^[a-z0-9](?:[a-z0-9-]{0,37}[a-z0-9])?\/[a-z0-9][a-z0-9_-]*$/;
const VERSION_PATTERN = /^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.+-]*)?$/;

const params = new URLSearchParams(window.location.search);
const packageName = params.get("name") ?? "";
const version = params.get("version") ?? "";
const sourceUrl = `/api/v1/user/source/${packageName}/${version}`;

interface Slice {
    bytes: Uint8Array<ArrayBuffer>;
    /** The whole archive's size, from the Content-Range total. */
    size: number;
}

type SliceResult = { ok: true; slice: Slice } | { ok: false; status: number };

// One ranged read. Anything but a coherent 206 - including a body that
// disagrees with its own Content-Range - reports a failure; status 0
// stands for "unreachable or incoherent".
async function fetchRange(range: string): Promise<SliceResult> {
    let response: Response;
    try {
        response = await fetch(sourceUrl, {
            credentials: "same-origin",
            headers: { Range: range },
        });
    } catch {
        return { ok: false, status: 0 };
    }
    if (response.status !== 206) {
        return { ok: false, status: response.status };
    }
    const span = parseContentRange(response.headers.get("Content-Range"));
    if (!span) {
        return { ok: false, status: 0 };
    }
    // The connection can still fail while the body streams in; that is
    // the same "unreachable" outcome as a failed fetch.
    let buffer: ArrayBuffer;
    try {
        buffer = await response.arrayBuffer();
    } catch {
        return { ok: false, status: 0 };
    }
    const bytes = new Uint8Array(buffer);
    if (bytes.length !== span.last - span.first + 1) {
        return { ok: false, status: 0 };
    }
    return { ok: true, slice: { bytes, size: span.size } };
}

// A span larger than one request's cap arrives in chunks. Deflated
// spans genuinely need this: compression level is producer-only, so a
// valid stream can be arbitrarily inefficient and exceed the cap even
// under the display cap.
async function fetchSpan(start: number, length: number): Promise<SliceResult> {
    const parts: Uint8Array<ArrayBuffer>[] = [];
    let size = 0;
    for (let at = start; at < start + length; at += MAX_REQUEST_BYTES) {
        const end = Math.min(at + MAX_REQUEST_BYTES, start + length);
        const result = await fetchRange(`bytes=${at}-${end - 1}`);
        if (!result.ok) {
            return result;
        }
        if (result.slice.bytes.length !== end - at) {
            return { ok: false, status: 0 };
        }
        parts.push(result.slice.bytes);
        size = result.slice.size;
    }
    const bytes = new Uint8Array(length);
    let at = 0;
    for (const part of parts) {
        bytes.set(part, at);
        at += part.length;
    }
    return { ok: true, slice: { bytes, size } };
}

// Decompresses a deflate-raw span, stopping shortly after `maxBytes`
// of output: enough to know the content was truncated without ever
// materializing a decompression bomb.
async function inflatePrefix(
    compressed: Uint8Array<ArrayBuffer>,
    maxBytes: number,
): Promise<Uint8Array> {
    const stream = new Blob([compressed])
        .stream()
        .pipeThrough(new DecompressionStream("deflate-raw"));
    const reader = stream.getReader();
    const parts: Uint8Array[] = [];
    let total = 0;
    while (total <= maxBytes) {
        const { done, value } = await reader.read();
        if (done) {
            break;
        }
        parts.push(value);
        total += value.length;
    }
    if (total > maxBytes) {
        await reader.cancel();
    }
    const bytes = new Uint8Array(total);
    let at = 0;
    for (const part of parts) {
        bytes.set(part, at);
        at += part.length;
    }
    return bytes;
}

function setText(root: HTMLElement, selector: string, text: string): void {
    const target = root.querySelector(selector);
    if (target instanceof HTMLElement) {
        target.textContent = text;
    }
}

interface FileView {
    notice: string | null;
    content: string | null;
}

function renderFileView(root: HTMLElement, view: FileView): void {
    const notice = root.querySelector("[data-source-notice]");
    if (notice instanceof HTMLElement) {
        notice.hidden = view.notice === null;
        notice.textContent = view.notice ?? "";
    }
    const pre = root.querySelector("[data-source-pre]");
    if (pre instanceof HTMLElement) {
        pre.hidden = view.content === null;
    }
    const content = root.querySelector("[data-source-content]");
    if (content instanceof HTMLElement) {
        content.textContent = view.content ?? "";
    }
}

// Serializes file loads: a click while an earlier file still streams
// must not let the slower response overwrite the newer selection.
let viewToken = 0;

async function viewFile(root: HTMLElement, entry: ArchiveEntry): Promise<void> {
    viewToken += 1;
    const token = viewToken;
    setText(root, "[data-source-file]", entry.name);
    setText(root, "[data-source-file-size]", formatBytes(entry.usize));
    renderFileView(root, { notice: "Loading…", content: null });

    const finish = (view: FileView) => {
        if (token === viewToken) {
            renderFileView(root, view);
        }
    };
    if (entry.method !== 0 && entry.method !== 8) {
        finish({
            notice: "This entry is outside the archive profile.",
            content: null,
        });
        return;
    }
    if (entry.csize === 0 || entry.usize === 0) {
        finish({ notice: "This file is empty.", content: null });
        return;
    }

    // Stored entries are their own content, so only the displayed
    // prefix is fetched; deflate must decompress from the start, so its
    // whole compressed span is fetched (bounded by the archive-size
    // cap) and decompression stops at the display cap.
    let bytes: Uint8Array;
    if (entry.method === 0) {
        const wanted = Math.min(entry.csize, MAX_DISPLAY_BYTES + 1);
        const result = await fetchSpan(entry.dataStart, wanted);
        if (!result.ok) {
            finish({ notice: fileLoadNotice(result.status), content: null });
            return;
        }
        bytes = result.slice.bytes;
    } else {
        const result = await fetchSpan(entry.dataStart, entry.csize);
        if (!result.ok) {
            finish({ notice: fileLoadNotice(result.status), content: null });
            return;
        }
        try {
            bytes = await inflatePrefix(result.slice.bytes, MAX_DISPLAY_BYTES);
        } catch {
            finish({
                notice: "This entry's compressed data does not decompress.",
                content: null,
            });
            return;
        }
    }

    const truncated =
        entry.usize > MAX_DISPLAY_BYTES || bytes.length > MAX_DISPLAY_BYTES;
    if (!truncated && bytes.length !== entry.usize) {
        finish({
            notice: "This entry's data disagrees with its declared size.",
            content: null,
        });
        return;
    }
    const text = decodeText(bytes.subarray(0, MAX_DISPLAY_BYTES), truncated);
    if (text === null) {
        finish({
            notice: `This is a binary file (${formatBytes(entry.usize)}); the viewer shows text only.`,
            content: null,
        });
        return;
    }
    finish({
        notice: truncated
            ? `Truncated: showing the first ${formatBytes(MAX_DISPLAY_BYTES)} of ${formatBytes(entry.usize)}.`
            : null,
        content: text,
    });
}

function fileLoadNotice(status: number): string {
    if (status === 401) {
        return "Your session expired; sign in again to keep browsing.";
    }
    return "This file could not be read from the registry.";
}

function renderDirectory(
    directory: TreeDirectory,
    templates: { directory: HTMLTemplateElement; file: HTMLTemplateElement },
    onSelect: (entry: ArchiveEntry, button: HTMLButtonElement) => void,
    fileButtons: Map<
        string,
        { entry: ArchiveEntry; button: HTMLButtonElement }
    >,
): DocumentFragment {
    const fragment = document.createDocumentFragment();
    for (const [name, child] of directory.directories) {
        const item = templates.directory.content.cloneNode(
            true,
        ) as DocumentFragment;
        const summary = item.querySelector('[data-slot="name"]');
        if (summary instanceof HTMLElement) {
            summary.textContent = `${name}/`;
        }
        const children = item.querySelector('[data-slot="children"]');
        if (children instanceof HTMLElement) {
            children.append(
                renderDirectory(child, templates, onSelect, fileButtons),
            );
        }
        fragment.append(item);
    }
    for (const entry of directory.files) {
        const item = templates.file.content.cloneNode(true) as DocumentFragment;
        const button = item.querySelector('[data-slot="file"]');
        if (button instanceof HTMLButtonElement) {
            button.textContent = entry.name.split("/").at(-1) ?? entry.name;
            button.addEventListener("click", () => onSelect(entry, button));
            fileButtons.set(entry.name, { entry, button });
        }
        fragment.append(item);
    }
    return fragment;
}

// The central directory usually rides in the tail read; otherwise it is
// fetched by its exact span.
async function centralDirectory(
    tail: Slice,
    layout: ArchiveLayout,
): Promise<Uint8Array | null> {
    const tailStart = tail.size - tail.bytes.length;
    if (layout.cdOffset >= tailStart) {
        const from = layout.cdOffset - tailStart;
        return tail.bytes.subarray(from, from + layout.cdSize);
    }
    const result = await fetchSpan(layout.cdOffset, layout.cdSize);
    return result.ok ? result.slice.bytes : null;
}

bootAccountShell(async (shell) => {
    if (!NAME_PATTERN.test(packageName) || !VERSION_PATTERN.test(version)) {
        shell.show(
            "error",
            "the source viewer needs a published package and version, " +
                "reached from a package's version list on the dashboard",
        );
        return;
    }
    setText(shell.root, "[data-source-package]", packageName);
    setText(shell.root, "[data-source-version]", version);

    const tail = await fetchRange(`bytes=-${TAIL_BYTES}`);
    if (!tail.ok) {
        if (tail.status === 401) {
            shell.show("signed-out");
        } else if (tail.status === 404) {
            shell.show(
                "error",
                "this version is not browsable: only verified versions are",
            );
        } else {
            shell.show(
                "error",
                "the archive could not be read from the registry",
            );
        }
        return;
    }
    const tree = shell.root.querySelector("[data-source-tree]");
    const directoryTemplate = document.getElementById("source-dir-template");
    const fileTemplate = document.getElementById("source-file-template");
    if (
        !(tree instanceof HTMLElement) ||
        !(directoryTemplate instanceof HTMLTemplateElement) ||
        !(fileTemplate instanceof HTMLTemplateElement)
    ) {
        return;
    }

    let entries: ArchiveEntry[];
    try {
        const layout = parseEocd(tail.slice.bytes, tail.slice.size);
        const cd = await centralDirectory(tail.slice, layout);
        if (cd === null) {
            shell.show(
                "error",
                "the archive could not be read from the registry",
            );
            return;
        }
        entries = parseCentralDirectory(cd, layout);
    } catch {
        shell.show("error", "the archive does not parse as a package archive");
        return;
    }

    let selected: HTMLButtonElement | null = null;
    const select = (entry: ArchiveEntry, button: HTMLButtonElement) => {
        selected?.removeAttribute("aria-current");
        selected?.classList.remove("text-steel");
        button.setAttribute("aria-current", "true");
        button.classList.add("text-steel");
        selected = button;
        void viewFile(shell.root, entry);
    };
    const fileButtons = new Map<
        string,
        { entry: ArchiveEntry; button: HTMLButtonElement }
    >();
    tree.replaceChildren(
        renderDirectory(
            buildTree(entries),
            { directory: directoryTemplate, file: fileTemplate },
            select,
            fileButtons,
        ),
    );
    shell.show("content");
    // The profile guarantees a root manifest; opening it beats an
    // empty pane.
    const manifest = fileButtons.get("cabin.toml");
    if (manifest) {
        select(manifest.entry, manifest.button);
    }
});
