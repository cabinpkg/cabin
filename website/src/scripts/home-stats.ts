// Fills the homepage's registry stats band from the public stats
// endpoint. Every figure in the band is registry data, so the band
// ships hidden and is revealed only after a successful fetch - a
// script-less or offline view drops the band entirely rather than
// showing an empty strip or placeholder dashes.
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
        band.hidden = false;
    });
}
