// DOM glue for the account pages' shared shell (AccountShell.astro):
// finds the shell's state sections and switches between them. The page
// scripts drive it - resolve the shared auth probe, render into the
// hidden content section, then reveal it; and fall back to the
// signed-out state whenever a later call answers 401.

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

export function accountShell(): Shell | null {
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
