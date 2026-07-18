// Progressive enhancement for docs pages (bundled, so it satisfies the CSP
// `script-src 'self'`): clicking a heading copies its deep link, and the
// on-page table of contents highlights the section currently in view.

import { DOCS_HIGHLIGHT_PARAM } from "../lib/constants";

setupHeadingAnchors();
setupTocScrollSpy();
setupScrollableTables();
setupCodeCopyButtons();
setupSearchHighlight();

function setupHeadingAnchors(): void {
    const anchors =
        document.querySelectorAll<HTMLAnchorElement>("a.heading-anchor");
    for (const anchor of anchors) {
        // The build labels anchors as plain section links (they also render
        // on pages without this script); claim the copy affordance only
        // where the handler below actually provides it.
        if (navigator.clipboard) {
            anchor.setAttribute("aria-label", "Copy link to this section");
        }
        anchor.addEventListener("click", (event) => {
            const href = anchor.getAttribute("href");
            if (!href || !navigator.clipboard) {
                return; // Fall back to normal in-page navigation.
            }
            event.preventDefault();
            const url = new URL(href, window.location.href).href;
            window.history.replaceState(null, "", href);
            navigator.clipboard.writeText(url).then(
                () => flashCopied(anchor),
                () => {},
            );
        });
    }
}

function flashCopied(anchor: HTMLAnchorElement): void {
    anchor.classList.add("heading-anchor-copied");
    announce("Link copied");
    window.setTimeout(() => {
        anchor.classList.remove("heading-anchor-copied");
    }, 1200);
}

// Mirror the visual "Link copied" affordance into a live region so screen
// readers announce the copy.
function announce(message: string): void {
    const status = document.querySelector("[data-copy-status]");
    if (!status) {
        return;
    }
    status.textContent = message;
    window.setTimeout(() => {
        status.textContent = "";
    }, 1200);
}

function setupTocScrollSpy(): void {
    const tocLinks = new Map<string, HTMLAnchorElement>();
    for (const link of document.querySelectorAll<HTMLAnchorElement>(
        "[data-toc-link]",
    )) {
        const id = link.dataset.tocLink;
        if (id) {
            tocLinks.set(id, link);
        }
    }
    if (tocLinks.size === 0) {
        return;
    }

    const headings: HTMLElement[] = [];
    for (const id of tocLinks.keys()) {
        const heading = document.getElementById(id);
        if (heading) {
            headings.push(heading);
        }
    }
    if (headings.length === 0) {
        return;
    }

    const setActive = (id: string): void => {
        for (const [linkId, link] of tocLinks) {
            link.classList.toggle("toc-link-active", linkId === id);
        }
    };

    // Clicking an entry pins it active until the reader scrolls.  A
    // short trailing section can't scroll up to the trigger line, so without
    // the pin the scroll math would re-highlight the section above it.
    let pinned = false;
    for (const [id, link] of tocLinks) {
        link.addEventListener("click", () => {
            pinned = true;
            setActive(id);
        });
    }
    const unpin = (): void => {
        pinned = false;
    };
    window.addEventListener("wheel", unpin, { passive: true });
    window.addEventListener("touchmove", unpin, { passive: true });
    window.addEventListener("keydown", unpin);

    let ticking = false;
    const update = (): void => {
        ticking = false;
        if (pinned) {
            return;
        }
        // The active section is the last heading scrolled past the header.
        let activeId = headings[0].id;
        for (const heading of headings) {
            if (heading.getBoundingClientRect().top - 100 <= 0) {
                activeId = heading.id;
            } else {
                break;
            }
        }
        // At the bottom of the page, force the last section active so a short
        // trailing section can still be highlighted.
        if (
            window.innerHeight + window.scrollY >=
            document.documentElement.scrollHeight - 1
        ) {
            activeId = headings[headings.length - 1].id;
        }
        setActive(activeId);
    };
    const onScroll = (): void => {
        if (!ticking) {
            ticking = true;
            window.requestAnimationFrame(update);
        }
    };

    window.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onScroll, { passive: true });
    update();
}

// On small screens wide tables become horizontally scrollable (see the
// `.prose table` rule in global.css).  A scroll region with clipped content must
// be keyboard-focusable to be reachable (WCAG 2.1.1), so mark the ones that
// overflow.
function setupScrollableTables(): void {
    const tables = document.querySelectorAll<HTMLTableElement>(".prose table");
    for (const table of tables) {
        if (table.scrollWidth > table.clientWidth) {
            table.setAttribute("tabindex", "0");
            table.setAttribute("role", "region");
            table.setAttribute("aria-label", "Scrollable table");
        }
    }
}

// When the reader arrives from a docs search result (`?highlight=<query>`),
// scroll the first matching term into view and highlight every occurrence.
// Search is document-level, so we highlight the query terms themselves across
// inline-code and formatting boundaries on these pages.
function setupSearchHighlight(): void {
    const raw = new URLSearchParams(window.location.search).get(
        DOCS_HIGHLIGHT_PARAM,
    );
    if (!raw) {
        return;
    }
    const container = document.querySelector<HTMLElement>(".docs-prose");
    if (!container) {
        return;
    }
    const terms = [
        ...new Set(
            raw
                .toLowerCase()
                .split(/\s+/)
                .filter((t) => t.length >= 2),
        ),
    ];
    if (terms.length === 0) {
        return;
    }

    const ranges = collectMatchRanges(container, terms);
    if (ranges.length === 0) {
        return;
    }
    ranges.sort((a, b) => a.compareBoundaryPoints(Range.START_TO_START, b));

    applyHighlight(ranges);
    scrollRangeIntoView(ranges[0]);
}

// Walk the article's text nodes and collect a Range for every case-insensitive
// occurrence of any search term, skipping injected UI like the copy buttons.
function collectMatchRanges(container: HTMLElement, terms: string[]): Range[] {
    const MAX_RANGES = 200;
    const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, {
        acceptNode(node) {
            const parent = node.parentElement;
            if (!parent || parent.closest("button, script, style")) {
                return NodeFilter.FILTER_REJECT;
            }
            return node.nodeValue?.trim()
                ? NodeFilter.FILTER_ACCEPT
                : NodeFilter.FILTER_REJECT;
        },
    });

    const ranges: Range[] = [];
    for (
        let node = walker.nextNode();
        node && ranges.length < MAX_RANGES;
        node = walker.nextNode()
    ) {
        const lower = (node.nodeValue ?? "").toLowerCase();
        for (const term of terms) {
            for (
                let idx = lower.indexOf(term);
                idx >= 0 && ranges.length < MAX_RANGES;
                idx = lower.indexOf(term, idx + term.length)
            ) {
                const range = document.createRange();
                range.setStart(node, idx);
                range.setEnd(node, idx + term.length);
                ranges.push(range);
            }
        }
    }
    return ranges;
}

interface HighlightRegistry {
    set(name: string, highlight: object): void;
}
type HighlightConstructor = new (...ranges: Range[]) => object;

// Prefer the CSS Custom Highlight API (no DOM mutation, styled via
// `::highlight(docs-search)` in global.css); fall back to wrapping the first
// match in a <mark> on browsers that lack it.
function applyHighlight(ranges: Range[]): void {
    const registry = (CSS as unknown as { highlights?: HighlightRegistry })
        .highlights;
    const HighlightCtor = (
        window as unknown as { Highlight?: HighlightConstructor }
    ).Highlight;
    if (registry && HighlightCtor) {
        registry.set("docs-search", new HighlightCtor(...ranges));
        return;
    }
    try {
        const mark = document.createElement("mark");
        mark.className = "search-highlight";
        ranges[0].surroundContents(mark);
    } catch {
        // The range spans element boundaries; skip the inline fallback.
    }
}

function scrollRangeIntoView(range: Range): void {
    const node = range.startContainer;
    const target = node instanceof Element ? node : node.parentElement;
    if (!target) {
        return;
    }
    const reduceMotion = window.matchMedia(
        "(prefers-reduced-motion: reduce)",
    ).matches;
    target.scrollIntoView({
        behavior: reduceMotion ? "auto" : "smooth",
        block: "center",
    });
}

// Add a "Copy" button to every code block.  The button lives in a wrapper next
// to the <pre> (not inside it) so it stays put while the code scrolls.
function setupCodeCopyButtons(): void {
    if (!navigator.clipboard) {
        return;
    }
    for (const pre of document.querySelectorAll<HTMLPreElement>(".prose pre")) {
        const code = pre.querySelector("code");
        if (!code) {
            continue;
        }
        const wrapper = document.createElement("div");
        wrapper.className = "code-block";
        pre.replaceWith(wrapper);
        wrapper.append(pre);

        const button = document.createElement("button");
        button.type = "button";
        button.className = "code-copy";
        button.textContent = "Copy";
        button.setAttribute("aria-label", "Copy code to clipboard");
        wrapper.append(button);

        button.addEventListener("click", () => {
            const lines = code.querySelectorAll(".line");
            const text =
                lines.length > 0
                    ? Array.from(lines, (line) => line.textContent).join("\n")
                    : (code.textContent ?? "");
            navigator.clipboard.writeText(text).then(
                () => {
                    button.textContent = "Copied";
                    button.classList.add("code-copy-done");
                    window.setTimeout(() => {
                        button.textContent = "Copy";
                        button.classList.remove("code-copy-done");
                    }, 1200);
                },
                () => {},
            );
        });
    }
}
