// Drives /settings/tokens: list, create, and revoke tokens over the
// session user API. The created token's plaintext exists only in this
// page's DOM - it is wiped on dismiss and on pagehide (so bfcache never
// re-shows it), and is never written to storage of any kind. A 401 from
// any call drops the whole page to the signed-out state.
import {
    asOutcome,
    createToken,
    type FetchLike,
    getTokens,
    revokeToken,
    sharedAuth,
    type TokenInfo,
} from "../lib/account.ts";
import { accountShell, type Shell } from "../lib/accountShell";
import { formatRelativeTime } from "../lib/format";

const doFetch: FetchLike = (input, init) => fetch(input, init);

const form = document.querySelector("[data-token-create]");
const nameInput = document.getElementById("token-name");
const submit = document.querySelector("[data-token-submit]");
const createNotice = document.querySelector("[data-create-notice]");
const createdPanel = document.querySelector("[data-created-panel]");
const createdToken = document.querySelector("[data-created-token]");
const copyButton = document.querySelector("[data-copy-token]");
const dismissButton = document.querySelector("[data-dismiss-token]");
const tokensNotice = document.querySelector("[data-tokens-notice]");
const tokensEmpty = document.querySelector("[data-tokens-empty]");
const tokensTable = document.querySelector("[data-tokens-table]");
const tokenRows = document.querySelector("[data-token-rows]");
const rowTemplate = document.getElementById("token-row-template");

function notify(target: Element | null, message: string | null): void {
    if (!(target instanceof HTMLElement)) {
        return;
    }
    target.textContent = message ?? "";
    target.hidden = message === null;
}

// The plaintext lives in this one element; wiping it is the whole
// "dismissed" semantic.
function wipePlaintext(): void {
    if (createdToken instanceof HTMLElement) {
        createdToken.textContent = "";
    }
    if (createdPanel instanceof HTMLElement) {
        createdPanel.hidden = true;
    }
    if (copyButton instanceof HTMLElement) {
        copyButton.textContent = "Copy";
    }
}

function renderRows(shell: Shell, tokens: TokenInfo[]): void {
    if (
        !(tokenRows instanceof HTMLElement) ||
        !(rowTemplate instanceof HTMLTemplateElement)
    ) {
        return;
    }
    if (tokensEmpty instanceof HTMLElement) {
        tokensEmpty.hidden = tokens.length > 0;
    }
    if (tokensTable instanceof HTMLElement) {
        tokensTable.hidden = tokens.length === 0;
    }
    tokenRows.replaceChildren();
    for (const token of tokens) {
        const row = rowTemplate.content.cloneNode(true) as DocumentFragment;
        const setSlot = (slot: string, text: string) => {
            const cell = row.querySelector(`[data-slot="${slot}"]`);
            if (cell instanceof HTMLElement) {
                cell.textContent = text;
            }
        };
        setSlot("name", token.name);
        setSlot("scopes", token.scopes.join(", ") || "—");
        setSlot("created", formatRelativeTime(token.created_at));
        setSlot(
            "last-used",
            token.last_used_at
                ? formatRelativeTime(token.last_used_at)
                : "never",
        );
        const revoked = row.querySelector('[data-slot="revoked"]');
        if (revoked instanceof HTMLElement) {
            revoked.hidden = !token.revoked;
        }
        const revoke = row.querySelector('[data-slot="revoke"]');
        if (revoke instanceof HTMLButtonElement) {
            revoke.hidden = token.revoked;
            revoke.addEventListener("click", () => {
                void handleRevoke(shell, revoke, token.id);
            });
        }
        tokenRows.append(row);
    }
}

// Refreshes the listing; failures land in the listing notice so a
// just-created plaintext panel survives a refresh hiccup. Returns false
// only when the session is gone.
async function refreshRows(shell: Shell): Promise<boolean> {
    const outcome = asOutcome(await getTokens(doFetch));
    if (outcome.state === "signed-out") {
        wipePlaintext();
        shell.show("signed-out");
        return false;
    }
    if (outcome.state === "failed") {
        notify(tokensNotice, outcome.message);
        return true;
    }
    notify(tokensNotice, null);
    renderRows(shell, outcome.data.tokens);
    return true;
}

async function handleRevoke(
    shell: Shell,
    button: HTMLButtonElement,
    id: string,
): Promise<void> {
    button.disabled = true;
    const outcome = asOutcome(await revokeToken(doFetch, id));
    if (outcome.state === "signed-out") {
        wipePlaintext();
        shell.show("signed-out");
        return;
    }
    if (outcome.state === "failed") {
        button.disabled = false;
        notify(tokensNotice, outcome.message);
        return;
    }
    await refreshRows(shell);
}

async function handleCreate(shell: Shell): Promise<void> {
    if (
        !(form instanceof HTMLFormElement) ||
        !(nameInput instanceof HTMLInputElement) ||
        !(submit instanceof HTMLButtonElement)
    ) {
        return;
    }
    const scopes = Array.from(
        form.querySelectorAll('input[name="scopes"]:checked'),
        (box) => (box instanceof HTMLInputElement ? box.value : ""),
    ).filter((scope) => scope !== "");

    submit.disabled = true;
    notify(createNotice, null);
    const outcome = asOutcome(
        await createToken(doFetch, nameInput.value, scopes),
    );
    submit.disabled = false;
    if (outcome.state === "signed-out") {
        wipePlaintext();
        shell.show("signed-out");
        return;
    }
    if (outcome.state === "failed") {
        notify(createNotice, outcome.message);
        return;
    }
    if (createdToken instanceof HTMLElement) {
        createdToken.textContent = outcome.data.token;
    }
    if (createdPanel instanceof HTMLElement) {
        createdPanel.hidden = false;
    }
    form.reset();
    // Refresh independently of the panel: if it fails, the plaintext
    // stays visible and the listing notice explains.
    await refreshRows(shell);
}

function wireControls(shell: Shell): void {
    if (form instanceof HTMLFormElement) {
        form.addEventListener("submit", (event) => {
            event.preventDefault();
            void handleCreate(shell);
        });
    }
    if (copyButton instanceof HTMLButtonElement) {
        copyButton.addEventListener("click", () => {
            const plaintext =
                createdToken instanceof HTMLElement
                    ? (createdToken.textContent ?? "")
                    : "";
            navigator.clipboard.writeText(plaintext).then(
                () => {
                    copyButton.textContent = "Copied";
                },
                () => {
                    notify(
                        createNotice,
                        "copying failed - select the token text and copy it manually",
                    );
                },
            );
        });
    }
    if (dismissButton instanceof HTMLButtonElement) {
        dismissButton.addEventListener("click", wipePlaintext);
    }
    // bfcache: never let a restored page re-show the plaintext.
    window.addEventListener("pagehide", wipePlaintext);
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
        const outcome = asOutcome(await getTokens(doFetch));
        if (outcome.state === "signed-out") {
            shell.show("signed-out");
            return;
        }
        if (outcome.state === "failed") {
            shell.show("error", outcome.message);
            return;
        }
        renderRows(shell, outcome.data.tokens);
        wireControls(shell);
        shell.show("content");
    });
}
