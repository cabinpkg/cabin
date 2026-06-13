import { getCollection } from "astro:content";
import type { APIRoute } from "astro";
import { buildDocsSearchItems } from "../lib/docsContent";

export const prerender = true;

export const GET: APIRoute = async () => {
    const entries = await getCollection("docs");
    const items = buildDocsSearchItems(entries);

    return new Response(JSON.stringify(items), {
        headers: {
            "content-type": "application/json; charset=utf-8",
        },
    });
};
