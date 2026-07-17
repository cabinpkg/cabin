// Fills the homepage's registry stats band from the public stats
// endpoint. The band itself is static (the build-time ports figure),
// and only the registry tiles ship hidden, revealed after a
// successful fetch - so a script-less or offline view is a clean
// static band, not placeholder dashes, and on sm+ viewports the
// reveal fills the already-rendered grid row without moving the page.
import { formatCount } from "../lib/format";
import { getRegistryStats } from "../lib/stats";

const band = document.querySelector("[data-registry-stats]");
if (band instanceof HTMLElement) {
    getRegistryStats().then((stats) => {
        if (!stats) {
            return;
        }
        const fill = (selector: string, value: number): void => {
            const target = band.querySelector(selector);
            if (target instanceof HTMLElement) {
                target.textContent = formatCount(value);
            }
        };
        fill("[data-stat-packages]", stats.packages);
        fill("[data-stat-versions]", stats.versions);
        fill("[data-stat-downloads]", stats.downloads);
        for (const tile of band.querySelectorAll("[data-stat-tile]")) {
            if (tile instanceof HTMLElement) {
                tile.hidden = false;
            }
        }
    });
}
