// The account-age refusal is the one sign-in refusal the registry
// annotates: /login/denied?reason=account-age&eligible=<YYYY-MM-DD>,
// the first UTC date on which the whole day is eligible
// (registry/docs/architecture.md, "Two credential planes"). Anyone can
// craft the URL, so the date is validated to the exact shape the
// registry emits before it is displayed; anything else falls back to
// the generic copy.
export function accountAgeEligibleDate(params: URLSearchParams): string | null {
    if (params.get("reason") !== "account-age") {
        return null;
    }
    const eligible = params.get("eligible") ?? "";
    return /^\d{4}-\d{2}-\d{2}$/.test(eligible) ? eligible : null;
}
