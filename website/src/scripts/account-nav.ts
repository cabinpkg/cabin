// Progressive enhancement for the header's account area: the static
// markup shows the signed-out "Sign in" affordance, and this swaps in the
// user's login name when the session-cookie probe answers signed-in. On
// error or with the registry unreachable the static default stays.
import { sharedAuth } from "../lib/account.ts";

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
    });
}
