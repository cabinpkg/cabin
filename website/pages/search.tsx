import type { GetServerSideProps } from "next";
import { supabaseServerClient } from "@supabase/supabase-auth-helpers/nextjs";
import { Center, Text } from "@chakra-ui/react";

import type { PackageOverview } from "~/utils/types";
import { PER_PAGE } from "~/utils/constants";
import type { Sort } from "~/components/SearchResult";
import SearchResult from "~/components/SearchResult";
import Meta from "~/components/Meta";

interface SearchProps {
    packages?: PackageOverview[];
    query: string;
    perPage: number;
    page: number;
    totalCount?: number;
}

export default function Search(props: SearchProps): JSX.Element {
    return (
        <>
            <Meta title={props.query.length === 0 ? "All Packages" : `Searching for '${props.query}'`} />
            {
                props.packages && props.totalCount ?
                    <SearchResult
                        packages={props.packages}
                        query={props.query}
                        current_path="/search"
                        perPage={props.perPage}
                        page={props.page}
                        totalCount={props.totalCount}
                    /> :
                    <Center><Text>no packages found</Text></Center>
            }
        </>
    );
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const query = context.query.query ?? "";
    const page = context.query.page ? +context.query.page : 1;
    const perPage = context.query.perPage ? +context.query.perPage : PER_PAGE;
    const sort: Sort = context.query.sort ? context.query.sort as Sort : "relevance";

    let request = supabaseServerClient(context)
        .rpc<PackageOverview>("get_uniq_packages", {}, { count: "exact" })
        .select("id, name, version, description, edition");
    if (query) {
        request = request.like("name", `%${query}%`);
    }
    if (sort === "newlyPublished") {
        request = request.order("published_at", { ascending: false });
    }

    const startIndex = (page - 1) * perPage;
    request = request.range(startIndex, startIndex + (perPage - 1));

    const { data, count, error } = await request;
    if (error) {
        console.error(error);
    }
    return {
        props: {
            packages: data,
            query,
            perPage,
            page,
            totalCount: count,
        },
    };
};
