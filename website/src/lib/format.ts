export function stringifyValue(value: unknown): string {
    return value === null || value === undefined ? "" : String(value);
}

export function formatEdition(edition: unknown): string {
    const value = stringifyValue(edition);
    return value === "" ? "" : `C++${value.slice(-2)}`;
}

export function formatRelativeTime(value: unknown): string {
    const time = Date.parse(stringifyValue(value));
    if (!Number.isFinite(time)) {
        return "";
    }

    const seconds = Math.round((time - Date.now()) / 1000);
    const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
        ["year", 31536000],
        ["month", 2592000],
        ["day", 86400],
        ["hour", 3600],
        ["minute", 60],
        ["second", 1],
    ];
    const formatter = new Intl.RelativeTimeFormat("en", {
        numeric: "auto",
    });

    for (const [unit, unitSeconds] of units) {
        if (Math.abs(seconds) >= unitSeconds || unit === "second") {
            return formatter.format(Math.round(seconds / unitSeconds), unit);
        }
    }

    return "";
}
