import type { APIRoute } from "astro";

export const prerender = true;

export const GET: APIRoute = () => {
    // RFC 9116 requires an Expires date; roll it a year past each build so a
    // deploy keeps the file current (trimmed to seconds for a clean RFC 3339 value).
    const expires = new Date();
    expires.setUTCFullYear(expires.getUTCFullYear() + 1);

    return new Response(
        [
            "Contact: https://github.com/cabinpkg/cabin/security/advisories/new",
            `Expires: ${expires.toISOString().slice(0, 19)}Z`,
            "Preferred-Languages: en",
            "Policy: https://github.com/cabinpkg/cabin/blob/main/SECURITY.md",
            "",
        ].join("\n"),
        {
            headers: {
                "content-type": "text/plain; charset=utf-8",
            },
        },
    );
};
