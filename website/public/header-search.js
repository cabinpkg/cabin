const siteSearchInput = document.getElementById("site-search");

if (
    siteSearchInput instanceof HTMLInputElement &&
    window.location.pathname.replace(/\/$/, "") ===
        siteSearchInput.dataset.searchPath
) {
    const query = new URLSearchParams(window.location.search).get("q");
    if (query !== null) {
        siteSearchInput.value = query;
    }
}
