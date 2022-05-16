import type { GetServerSideProps } from "next";
import { supabaseServerClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package as PackageType } from "~/utils/types";
import { PER_PAGE } from "~/utils/constants";
import type { Sort } from "~/components/SearchResult";
import SearchResult from "~/components/SearchResult";

interface SearchProps {
    packages: PackageType[];
    query: string;
    perPage: number;
    page: number;
    totalCount: number;
}

export default function Search(props: SearchProps): JSX.Element {
    return (
        <SearchResult
            packages={props.packages}
            query={props.query}
            pathname="/search"
            perPage={props.perPage}
            page={props.page}
            totalCount={props.totalCount}
        />
    );
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const query = context.query.query;
    const page = context.query.page ? +context.query.page : 1;
    const perPage = context.query.perPage ? +context.query.perPage : PER_PAGE;
    const sort: Sort = context.query.sort ? context.query.sort as Sort : "relevance";

    let request = supabaseServerClient(context)
        .rpc<PackageType>("get_uniq_packages", {}, { count: "exact" })
        .select("*"); // TODO: Improve selection: name, total downloads, updated_at, ...
    if (query) {
        request = request.like("name", `%${query}%`);
    }
    if (sort === "newlyPublished") {
        request = request.order("published_at", { ascending: false });
    }

    const startIndex = (page - 1) * perPage;
    request = request.range(startIndex, startIndex + (perPage - 1));

    const { data, count } = await request;
    if (data && count) {
        return {
            props: {
                packages: data,
                query,
                perPage,
                page,
                totalCount: count,
            },
        };
    }
    return {
        notFound: true,
    };
};
