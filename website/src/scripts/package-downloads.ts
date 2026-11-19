// Fills a package page's downloads from the registry's stats: the
// total in the Package Info card, then a column per UTC day and the
// same days as a table under a disclosure. Both ship
// hidden and are revealed only after a successful fetch; a script-less
// or offline view omits them.
import { formatCount } from "../lib/format";
import { getPackageDownloads } from "../lib/stats";

const SVG = "http://www.w3.org/2000/svg";
// The chart always spans the last 90 days, today at the right edge, so
// the days before counting started are empty space, not wider columns.
const SLOTS = 90;
// The columns' height in viewBox units; each day is 3 units wide, a
// 2-unit column and a 1-unit gap.
const HEIGHT = 40;

function element(name: string, attributes: Record<string, string>): Element {
    const node = document.createElementNS(SVG, name);
    for (const [key, value] of Object.entries(attributes)) {
        node.setAttribute(key, value);
    }
    return node;
}

const chart = document.querySelector("[data-package-downloads]");
const total = document.querySelector("[data-downloads-total]");
const name = chart instanceof HTMLElement ? chart.dataset.package : undefined;
if (chart instanceof HTMLElement && total instanceof HTMLElement && name) {
    getPackageDownloads(name).then((stats) => {
        if (!stats) {
            return;
        }
        const count = total.querySelector("[data-downloads-count]");
        if (count) {
            count.textContent = formatCount(stats.downloads);
        }
        total.hidden = false;

        const days = stats.days.slice(-SLOTS);
        const offset = SLOTS - days.length;
        const plot = chart.querySelector("[data-downloads-plot]");
        const rows = chart.querySelector("[data-downloads-rows]");
        const first = chart.querySelector("[data-downloads-first]");
        const last = chart.querySelector("[data-downloads-last]");
        if (days.length === 0 || !plot || !rows || !first || !last) {
            return;
        }
        const peak = Math.max(1, ...days.map((entry) => entry.downloads));
        const svg = element("svg", {
            viewBox: `0 0 ${SLOTS * 3} ${HEIGHT + 1}`,
            preserveAspectRatio: "none",
            "aria-hidden": "true",
            class: "h-full w-full",
        });
        days.forEach(({ day, downloads }, index) => {
            const column = element("g", { class: "group" });
            const title = element("title", {});
            title.textContent = `${day}: ${formatCount(downloads)}`;
            const height = (downloads / peak) * HEIGHT;
            column.append(
                title,
                // The whole slot is the hover target, so a quiet day has one too.
                element("rect", {
                    x: String((offset + index) * 3),
                    y: "0",
                    width: "3",
                    height: String(HEIGHT),
                    class: "fill-transparent",
                }),
                element("rect", {
                    x: String((offset + index) * 3),
                    y: String(HEIGHT - height),
                    width: "2",
                    height: String(height),
                    class: "fill-steel group-hover:fill-steel-bright",
                }),
            );
            svg.append(column);

            const row = document.createElement("tr");
            const date = document.createElement("th");
            date.scope = "row";
            date.className = "text-left font-normal";
            date.textContent = day;
            const count = document.createElement("td");
            count.textContent = formatCount(downloads);
            row.append(date, count);
            rows.append(row);
        });
        svg.append(
            element("line", {
                x1: "0",
                y1: String(HEIGHT + 0.5),
                x2: String(SLOTS * 3),
                y2: String(HEIGHT + 0.5),
                "vector-effect": "non-scaling-stroke",
                class: "stroke-line",
            }),
        );
        plot.append(svg);
        // The left edge is the first counted day only once the chart is full.
        if (days.length > 1) {
            first.textContent =
                offset > 0 ? `since ${days[0].day}` : days[0].day;
        }
        last.textContent = days[days.length - 1].day;
        chart.hidden = false;
    });
}
