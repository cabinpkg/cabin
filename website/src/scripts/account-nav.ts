// Progressive enhancement for the header's account area: the static
// markup shows the signed-out "Sign in" affordance, and this swaps in the
// account menu (a disclosure: Dashboard / API tokens / Profile / Sign
// out) when the session-cookie probe answers signed-in. On error or with
// the registry unreachable the static default stays.
import { type FetchLike, sharedAuth, signOut } from "../lib/account.ts";
import { loadAvatar } from "../lib/avatar.ts";

const doFetch: FetchLike = (input, init) => fetch(input, init);

function wireMenu(nav: HTMLElement): void {
    const button = nav.querySelector("[data-account-menu-button]");
    const menu = nav.querySelector("#account-menu");
    if (
        !(button instanceof HTMLButtonElement) ||
        !(menu instanceof HTMLElement)
    ) {
        return;
    }
    const setOpen = (open: boolean) => {
        menu.hidden = !open;
        button.setAttribute("aria-expanded", String(open));
    };
    button.addEventListener("click", () => {
        setOpen(button.getAttribute("aria-expanded") !== "true");
    });
    document.addEventListener("click", (event) => {
        if (
            !menu.hidden &&
            event.target instanceof Node &&
            !nav.contains(event.target)
        ) {
            setOpen(false);
        }
    });
    document.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && !menu.hidden) {
            setOpen(false);
            button.focus();
        }
    });

    const signout = nav.querySelector("[data-account-signout]");
    if (signout instanceof HTMLButtonElement) {
        signout.addEventListener("click", async () => {
            signout.disabled = true;
            const result = await signOut(doFetch);
            // A 401 means the session is already gone - that IS signed
            // out. Anything else failed; let the user retry.
            if (result.ok || result.status === 401) {
                window.location.assign("/");
                return;
            }
            signout.disabled = false;
            signout.textContent = "Sign out (retry)";
        });
    }
}

const nav = document.querySelector("[data-account-nav]");
if (nav instanceof HTMLElement) {
    sharedAuth().then((auth) => {
        if (auth.state !== "signed-in") {
            return;
        }
        const login = nav.querySelector("[data-account-login]");
        if (login instanceof HTMLElement) {
            login.textContent = auth.user.login;
        }
        // 2x the rendered 24px for high-density displays.
        loadAvatar(nav, auth.user, 48);
        for (const section of nav.querySelectorAll(
            "[data-account-signed-out]",
        )) {
            if (section instanceof HTMLElement) {
                section.hidden = true;
            }
        }
        for (const section of nav.querySelectorAll(
            "[data-account-signed-in]",
        )) {
            if (section instanceof HTMLElement) {
                section.hidden = false;
            }
        }
        wireMenu(nav);
    });
}
