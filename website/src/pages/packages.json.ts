import type { APIRoute } from "astro";
import { getPackageSearchIndex } from "../lib/packages";

export const prerender = true;

export const GET: APIRoute = async () => {
    const packages = await getPackageSearchIndex();

    return new Response(JSON.stringify(packages), {
        headers: {
            "content-type": "application/json; charset=utf-8",
        },
    });
};
