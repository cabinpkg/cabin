// Drives /dashboard/package: settles the shared auth probe, then loads
// one visible package's verified versions and newest-version runtime
// dependencies plus its reverse dependents from the session package
// routes. Everything renders via textContent - never markup.
import {
    asOutcome,
    type FetchLike,
    getPackageDetail,
    getReverseDependencies,
    type PackageDetail,
    type ReverseDependent,
    sharedAuth,
} from "../lib/account.ts";
import { accountShell } from "../lib/accountShell";
import { ACCOUNT_URLS } from "../lib/constants";
import { formatCount, formatRelativeTime } from "../lib/format";

// The package-name grammar, mirrored from the registry's route
// validation: everything it admits is URL-path-safe verbatim, so the
// API paths are interpolated unencoded (percent-escapes would fail the
// server's charset checks).
const NAME_PATTERN =
    /^[a-z0-9](?:[a-z0-9-]{0,37}[a-z0-9])?\/[a-z0-9][a-z0-9_-]*$/;

const doFetch: FetchLike = (input, init) => fetch(input, init);

const params = new URLSearchParams(window.location.search);
const packageName = params.get("name") ?? "";

function detailHref(full: string): string {
    return `${ACCOUNT_URLS.package}?name=${encodeURIComponent(full)}`;
}

function setText(root: HTMLElement, selector: string, text: string): void {
    const target = root.querySelector(selector);
    if (target instanceof HTMLElement) {
        target.textContent = text;
    }
}

function renderVersions(root: HTMLElement, detail: PackageDetail): void {
    const list = root.querySelector("[data-package-versions]");
    const template = document.getElementById("package-version-template");
    if (
        !(list instanceof HTMLElement) ||
        !(template instanceof HTMLTemplateElement)
    ) {
        return;
    }
    const full = `${detail.scope}/${detail.name}`;
    list.replaceChildren();
    for (const version of detail.versions) {
        const row = template.content.cloneNode(true) as DocumentFragment;
        const number = row.querySelector('[data-slot="version"]');
        if (number instanceof HTMLElement) {
            number.textContent = version.version;
        }
        const yanked = row.querySelector('[data-slot="yanked"]');
        if (yanked instanceof HTMLElement) {
            yanked.hidden = !version.yanked;
        }
        const source = row.querySelector('[data-slot="source"]');
        if (source instanceof HTMLAnchorElement) {
            source.href =
                `${ACCOUNT_URLS.source}?name=${encodeURIComponent(full)}` +
                `&version=${encodeURIComponent(version.version)}`;
        }
        const downloads = row.querySelector('[data-slot="downloads"]');
        if (downloads instanceof HTMLElement) {
            downloads.textContent = `${formatCount(version.downloads)} downloads`;
        }
        const published = row.querySelector('[data-slot="published"]');
        if (published instanceof HTMLElement) {
            published.textContent = formatRelativeTime(version.published_at);
        }
        list.append(row);
    }
}

function renderDependencies(root: HTMLElement, detail: PackageDetail): void {
    setText(
        root,
        "[data-dependencies-note]",
        `Runtime dependencies of ${detail.newest_version}, the newest version.`,
    );
    const list = root.querySelector("[data-package-dependencies]");
    const empty = root.querySelector("[data-dependencies-empty]");
    const template = document.getElementById("package-dependency-template");
    if (
        !(list instanceof HTMLElement) ||
        !(template instanceof HTMLTemplateElement)
    ) {
        return;
    }
    const entries = Object.entries(detail.dependencies);
    if (empty instanceof HTMLElement) {
        empty.hidden = entries.length > 0;
    }
    list.replaceChildren();
    for (const [name, requirement] of entries) {
        const row = template.content.cloneNode(true) as DocumentFragment;
        // Only a canonical scoped name has a detail page to link to; a
        // key outside the grammar renders as plain text.
        const slot = NAME_PATTERN.test(name) ? "link" : "name";
        const target = row.querySelector(`[data-slot="${slot}"]`);
        if (target instanceof HTMLElement) {
            target.textContent = name;
            target.hidden = false;
            if (target instanceof HTMLAnchorElement) {
                target.href = detailHref(name);
            }
        }
        const version = row.querySelector('[data-slot="requirement"]');
        if (version instanceof HTMLElement) {
            version.textContent = requirement;
        }
        list.append(row);
    }
}

function renderDependents(
    root: HTMLElement,
    dependents: ReverseDependent[],
): void {
    const list = root.querySelector("[data-package-dependents]");
    const empty = root.querySelector("[data-dependents-empty]");
    const template = document.getElementById("package-dependent-template");
    if (
        !(list instanceof HTMLElement) ||
        !(template instanceof HTMLTemplateElement)
    ) {
        return;
    }
    if (empty instanceof HTMLElement) {
        empty.hidden = dependents.length > 0;
    }
    list.replaceChildren();
    for (const dependent of dependents) {
        const row = template.content.cloneNode(true) as DocumentFragment;
        const full = `${dependent.scope}/${dependent.name}`;
        const link = row.querySelector('[data-slot="link"]');
        if (link instanceof HTMLAnchorElement) {
            link.textContent = full;
            link.href = detailHref(full);
        }
        const versions = row.querySelector('[data-slot="versions"]');
        if (versions instanceof HTMLElement) {
            const count = dependent.matching_versions;
            versions.textContent =
                `${count} ${count === 1 ? "version" : "versions"}, ` +
                `newest ${dependent.newest_matching_version}`;
        }
        list.append(row);
    }
}

const shell = accountShell();
if (shell) {
    sharedAuth().then(async (auth) => {
        if (auth.state === "signed-out") {
            shell.show("signed-out");
            return;
        }
        if (auth.state === "error") {
            shell.show("error", auth.message);
            return;
        }
        if (auth.state !== "signed-in") {
            return;
        }
        if (!NAME_PATTERN.test(packageName)) {
            shell.show("error", "missing or malformed package name");
            return;
        }
        setText(shell.root, "[data-package-name]", packageName);
        const slash = packageName.indexOf("/");
        const scope = packageName.slice(0, slash);
        const name = packageName.slice(slash + 1);
        const [detail, dependents] = await Promise.all([
            getPackageDetail(doFetch, scope, name),
            getReverseDependencies(doFetch, scope, name),
        ]);
        const detailOutcome = asOutcome(detail);
        const dependentsOutcome = asOutcome(dependents);
        if (
            detailOutcome.state === "signed-out" ||
            dependentsOutcome.state === "signed-out"
        ) {
            shell.show("signed-out");
            return;
        }
        // Both routes share the same visibility gate, so a 404 means
        // the package has no verified versions (or does not exist) -
        // the two are indistinguishable by design.
        if (!detail.ok && detail.status === 404) {
            shell.show("error", "this package has no verified versions");
            return;
        }
        if (detailOutcome.state === "failed") {
            shell.show("error", detailOutcome.message);
            return;
        }
        if (dependentsOutcome.state === "failed") {
            shell.show("error", dependentsOutcome.message);
            return;
        }
        renderVersions(shell.root, detailOutcome.data);
        renderDependencies(shell.root, detailOutcome.data);
        renderDependents(shell.root, dependentsOutcome.data.dependents);
        shell.show("content");
    });
}
