// Swaps the denied page's generic copy for the account-age message
// when the registry's refusal names it (and a valid eligible date
// rides along). The page stays fully static without the script or the
// params - the generic copy, which must render even when the registry
// is unreachable, is the fallback.
import { accountAgeEligibleDate } from "../lib/loginDenied";

const eligible = accountAgeEligibleDate(
    new URLSearchParams(window.location.search),
);
if (eligible) {
    const date = document.querySelector("[data-eligible-date]");
    if (date instanceof HTMLElement) {
        date.textContent = eligible;
    }
    for (const generic of document.querySelectorAll("[data-denied-generic]")) {
        if (generic instanceof HTMLElement) {
            generic.hidden = true;
        }
    }
    const specific = document.querySelector("[data-denied-account-age]");
    if (specific instanceof HTMLElement) {
        specific.hidden = false;
    }
}
