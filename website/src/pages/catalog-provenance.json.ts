import type { APIRoute } from "astro";
import { getCatalog } from "../lib/packages";

export const prerender = true;

// Which catalog this build rendered (scripts/fetch-catalog.mjs), so a
// deployed site can be compared with what the registry serves now.
export const GET: APIRoute = () =>
    new Response(JSON.stringify(getCatalog().provenance), {
        headers: { "content-type": "application/json; charset=utf-8" },
    });
