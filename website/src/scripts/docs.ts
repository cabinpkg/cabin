// Progressive enhancement for docs pages (bundled, so it satisfies the CSP
// `script-src 'self'`): clicking a heading copies its deep link, and the
// on-page table of contents highlights the section currently in view.

setupHeadingAnchors();
setupTocScrollSpy();
setupScrollableTables();
setupCodeCopyButtons();

function setupHeadingAnchors(): void {
    const anchors =
        document.querySelectorAll<HTMLAnchorElement>("a.heading-anchor");
    for (const anchor of anchors) {
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

    // Clicking an entry pins it active until the reader actually scrolls. A
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
// `.prose table` rule in global.css). A scroll region with clipped content must
// be keyboard-focusable to be reachable (WCAG 2.1.1), so mark the ones that
// actually overflow.
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

// Add a "Copy" button to every code block. The button lives in a wrapper next
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
