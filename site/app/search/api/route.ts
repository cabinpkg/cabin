import { cache } from "react";

import { PER_PAGE } from "~/app/_lib/constants";
import { getHasuraClient } from "../../_lib/hasuraClient";

const searchPackages = cache(
    async (name: string, limit: number, offset: number) => {
        const hasuraClient = getHasuraClient();
        const data = await hasuraClient.searchPackages({
            name: `%${name}%`,
            limit,
            offset,
        });
        return data;
    },
);

export async function GET(request: Request): Promise<Response> {
    const { searchParams } = new URL(request.url);
    const name = searchParams.get("q") ?? "";
    const page = Number(searchParams.get("page") ?? 1);
    const perPage = Number(searchParams.get("perPage") ?? PER_PAGE);

    const data = await searchPackages(name, perPage, (page - 1) * perPage);
    console.log("GET /search/api:", data);

    return Response.json(data);
}
