// Drives /dashboard: settles the shared auth probe, then loads usage and
// the package list from the session user API. A 401 from any call drops
// the whole page to the signed-out state (never stale account data).
import {
    type AccountPackage,
    asOutcome,
    type FetchLike,
    getPackages,
    getUsage,
    type SearchHit,
    searchPackages,
    type Usage,
} from "../lib/account.ts";
import { bootAccountShell } from "../lib/accountShell";
import { ACCOUNT_URLS } from "../lib/constants";
import { debounce } from "../lib/debounce";
import { formatBytes, formatCount, formatRelativeTime } from "../lib/format";

const SEARCH_DEBOUNCE_MS = 200;

const doFetch: FetchLike = (input, init) => fetch(input, init);

function setText(root: HTMLElement, selector: string, text: string): void {
    const target = root.querySelector(selector);
    if (target instanceof HTMLElement) {
        target.textContent = text;
    }
}

function setBar(
    root: HTMLElement,
    selector: string,
    used: number,
    max: number,
): void {
    const bar = root.querySelector(selector);
    if (!(bar instanceof HTMLElement) || max <= 0) {
        return;
    }
    const percent = Math.min(100, Math.round((used / max) * 100));
    bar.setAttribute("aria-valuenow", String(percent));
    const fill = bar.querySelector("[data-fill]");
    if (fill instanceof HTMLElement) {
        fill.style.width = `${percent}%`;
    }
}

// Only the two metrics with a matching quota get a progress bar;
// published_today has no compatible denominator in the usage payload.
function renderUsage(root: HTMLElement, usage: Usage): void {
    setText(root, "[data-usage-packages]", String(usage.package_count));
    setText(
        root,
        "[data-usage-packages-quota]",
        `of ${usage.quotas.max_packages_total} on the ${usage.quota_class} quota class`,
    );
    setBar(
        root,
        "[data-usage-packages-bar]",
        usage.package_count,
        usage.quotas.max_packages_total,
    );
    setText(root, "[data-usage-storage]", formatBytes(usage.stored_bytes));
    setText(
        root,
        "[data-usage-storage-quota]",
        `of ${formatBytes(usage.quotas.max_total_bytes_per_user)}`,
    );
    setBar(
        root,
        "[data-usage-storage-bar]",
        usage.stored_bytes,
        usage.quotas.max_total_bytes_per_user,
    );
    setText(
        root,
        "[data-usage-published-today]",
        String(usage.published_today),
    );
    setText(root, "[data-usage-verified]", String(usage.versions.verified));
    setText(root, "[data-usage-pending]", String(usage.versions.pending));
    setText(root, "[data-usage-rejected]", String(usage.versions.rejected));
}

function packageDownloads(pkg: AccountPackage): number {
    return pkg.versions.reduce((sum, version) => sum + version.downloads, 0);
}

// The usage payload carries no download figure; the packages payload is
// complete (unpaginated), so the dashboard total is summed client-side.
// That payload is the created-packages list, so the card is labeled
// accordingly - unlike the published_by-keyed usage figures, a version
// a scope-mate published into your package counts here, not there.
function renderDownloadTotal(
    root: HTMLElement,
    packages: AccountPackage[],
): void {
    const total = packages.reduce((sum, pkg) => sum + packageDownloads(pkg), 0);
    setText(root, "[data-usage-downloads]", formatCount(total));
}

function renderPackages(root: HTMLElement, packages: AccountPackage[]): void {
    const list = root.querySelector("[data-packages-list]");
    const empty = root.querySelector("[data-packages-empty]");
    const packageTemplate = document.getElementById("package-template");
    const versionTemplate = document.getElementById("version-template");
    if (
        !(list instanceof HTMLElement) ||
        !(packageTemplate instanceof HTMLTemplateElement) ||
        !(versionTemplate instanceof HTMLTemplateElement)
    ) {
        return;
    }
    if (empty instanceof HTMLElement) {
        empty.hidden = packages.length > 0;
    }
    list.replaceChildren();
    for (const pkg of packages) {
        const item = packageTemplate.content.cloneNode(
            true,
        ) as DocumentFragment;
        const name = item.querySelector('[data-slot="name"]');
        if (name instanceof HTMLElement) {
            name.textContent = pkg.name;
        }
        const total = item.querySelector('[data-slot="package-downloads"]');
        if (total instanceof HTMLElement) {
            total.textContent = `${formatCount(packageDownloads(pkg))} downloads`;
        }
        const versions = item.querySelector('[data-slot="versions"]');
        if (versions instanceof HTMLElement) {
            for (const version of pkg.versions) {
                const row = versionTemplate.content.cloneNode(
                    true,
                ) as DocumentFragment;
                const number = row.querySelector('[data-slot="version"]');
                if (number instanceof HTMLElement) {
                    number.textContent = version.version;
                }
                const badge = row.querySelector(
                    `[data-slot="${version.verification}"]`,
                );
                if (badge instanceof HTMLElement) {
                    badge.hidden = false;
                }
                const yanked = row.querySelector('[data-slot="yanked"]');
                if (yanked instanceof HTMLElement) {
                    yanked.hidden = !version.yanked;
                }
                const downloads = row.querySelector('[data-slot="downloads"]');
                // Pending and rejected versions were never downloadable;
                // a "0 downloads" there would read as a lifetime figure.
                if (
                    downloads instanceof HTMLElement &&
                    version.verification === "verified"
                ) {
                    downloads.textContent = `${formatCount(version.downloads)} downloads`;
                }
                const source = row.querySelector('[data-slot="source"]');
                // Only verified versions are browsable (the source route
                // gates on verified exactly like the artifact route).
                if (
                    source instanceof HTMLAnchorElement &&
                    version.verification === "verified"
                ) {
                    source.href =
                        `${ACCOUNT_URLS.source}?name=${encodeURIComponent(pkg.name)}` +
                        `&version=${encodeURIComponent(version.version)}`;
                    source.hidden = false;
                }
                const published = row.querySelector('[data-slot="published"]');
                if (published instanceof HTMLElement) {
                    published.textContent = formatRelativeTime(
                        version.published_at,
                    );
                }
                versions.append(row);
            }
        }
        list.append(item);
    }
}

function renderSearchHits(list: HTMLElement, hits: SearchHit[]): void {
    const template = document.getElementById("search-hit-template");
    if (!(template instanceof HTMLTemplateElement)) {
        return;
    }
    list.replaceChildren();
    for (const hit of hits) {
        const item = template.content.cloneNode(true) as DocumentFragment;
        const full = `${hit.scope}/${hit.name}`;
        const link = item.querySelector('[data-slot="link"]');
        if (link instanceof HTMLAnchorElement) {
            link.href = `${ACCOUNT_URLS.package}?name=${encodeURIComponent(full)}`;
        }
        const name = item.querySelector('[data-slot="name"]');
        if (name instanceof HTMLElement) {
            name.textContent = full;
        }
        const version = item.querySelector('[data-slot="version"]');
        if (version instanceof HTMLElement) {
            version.textContent = hit.version;
        }
        const yanked = item.querySelector('[data-slot="yanked"]');
        if (yanked instanceof HTMLElement) {
            yanked.hidden = !hit.yanked;
        }
        const downloads = item.querySelector('[data-slot="downloads"]');
        if (downloads instanceof HTMLElement) {
            downloads.textContent = `${formatCount(hit.downloads)} downloads`;
        }
        list.append(item);
    }
}

// Wires the package search box: debounced, and generation-guarded so a
// slow response for an older query can never overwrite a newer one's
// results. A 401 mid-search drops the whole page to signed-out via
// `onSignedOut`, like every other account call.
function wireSearch(root: HTMLElement, onSignedOut: () => void): void {
    const input = root.querySelector("[data-search-input]");
    const list = root.querySelector("[data-search-results]");
    const status = root.querySelector("[data-search-status]");
    if (
        !(input instanceof HTMLInputElement) ||
        !(list instanceof HTMLElement) ||
        !(status instanceof HTMLElement)
    ) {
        return;
    }
    const showStatus = (text: string) => {
        status.textContent = text;
        status.hidden = text === "";
    };
    // Bumped on every keystroke - not merely when the debounced run
    // starts - so a response in flight for an older query can never
    // render into the debounce window of a newer one.
    let generation = 0;
    const run = async () => {
        const query = input.value.trim();
        const current = generation;
        if (query === "") {
            list.replaceChildren();
            showStatus("");
            return;
        }
        const result = await searchPackages(doFetch, query);
        if (current !== generation) {
            return;
        }
        const outcome = asOutcome(result);
        if (outcome.state === "signed-out") {
            onSignedOut();
            return;
        }
        if (outcome.state === "failed") {
            list.replaceChildren();
            showStatus(outcome.message);
            return;
        }
        renderSearchHits(list, outcome.data.results);
        showStatus(
            outcome.data.results.length === 0 ? "No packages matched." : "",
        );
    };
    const debouncedRun = debounce(() => {
        void run();
    }, SEARCH_DEBOUNCE_MS);
    input.addEventListener("input", () => {
        generation += 1;
        debouncedRun();
    });
}

bootAccountShell(async (shell) => {
    const [usage, packages] = await Promise.all([
        getUsage(doFetch),
        getPackages(doFetch),
    ]);
    const usageOutcome = asOutcome(usage);
    const packagesOutcome = asOutcome(packages);
    if (
        usageOutcome.state === "signed-out" ||
        packagesOutcome.state === "signed-out"
    ) {
        shell.show("signed-out");
        return;
    }
    if (usageOutcome.state === "failed") {
        shell.show("error", usageOutcome.message);
        return;
    }
    if (packagesOutcome.state === "failed") {
        shell.show("error", packagesOutcome.message);
        return;
    }
    renderUsage(shell.root, usageOutcome.data);
    renderDownloadTotal(shell.root, packagesOutcome.data.packages);
    renderPackages(shell.root, packagesOutcome.data.packages);
    wireSearch(shell.root, () => shell.show("signed-out"));
    shell.show("content");
});
