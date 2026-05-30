import type { APIRoute } from "astro";
import { SITE_URL } from "../lib/constants";

export const prerender = true;

export const GET: APIRoute = () =>
    new Response(
        [
            "User-agent: *",
            "Allow: /",
            "",
            `Sitemap: ${new URL("/sitemap-index.xml", SITE_URL).toString()}`,
            "",
        ].join("\n"),
        {
            headers: {
                "content-type": "text/plain; charset=utf-8",
            },
        },
    );
