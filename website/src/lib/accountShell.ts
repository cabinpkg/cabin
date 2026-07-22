// DOM glue for the account pages' shared shell (AccountShell.astro):
// finds the shell's state sections and switches between them. Pages
// enter through bootAccountShell() - it settles the shared auth probe
// and maps signed-out / error onto the shell states, then hands a
// signed-in user to the page's render callback, which renders into
// the hidden content section and reveals it (falling back to the
// signed-out state whenever a later call answers 401).

import { sharedAuth, type User } from "./account.ts";

export type ShellState = "loading" | "signed-out" | "error" | "content";

export interface Shell {
    root: HTMLElement;
    show: (state: ShellState, message?: string) => void;
}

const SECTIONS: Record<ShellState, string> = {
    loading: "[data-shell-loading]",
    "signed-out": "[data-shell-signed-out]",
    error: "[data-shell-error]",
    content: "[data-shell-content]",
};

function accountShell(): Shell | null {
    const root = document.querySelector("[data-account-shell]");
    if (!(root instanceof HTMLElement)) {
        return null;
    }
    // A bfcache restoration resumes the page with its old DOM and module
    // state instead of re-running anything, so a revoked or expired
    // session would keep stale account data on screen. Reload to re-probe.
    window.addEventListener("pageshow", (event) => {
        if (event.persisted) {
            window.location.reload();
        }
    });
    return {
        root,
        show(state, message) {
            for (const [name, selector] of Object.entries(SECTIONS)) {
                const section = root.querySelector(selector);
                if (section instanceof HTMLElement) {
                    section.hidden = name !== state;
                }
            }
            if (state === "error") {
                const detail = root.querySelector("[data-shell-error-message]");
                if (detail instanceof HTMLElement) {
                    detail.textContent =
                        message ??
                        "something went wrong talking to the registry";
                }
            }
        },
    };
}

export function bootAccountShell(
    render: (shell: Shell, user: User) => void | Promise<void>,
): void {
    const shell = accountShell();
    if (!shell) {
        return;
    }
    sharedAuth().then((auth) => {
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
        return render(shell, auth.user);
    });
}
