import type { GetServerSideProps } from "next";
import { Center, Text } from "@chakra-ui/react";

import type { PackageOverview } from "~/utils/types";
import SearchResult from "~/components/SearchResult";
import Meta from "~/components/Meta";
import { BASE_API_URL, PER_PAGE } from "~/utils/constants";

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
            <Meta
                title={
                    props.query.length === 0
                        ? "All Packages"
                        : `Searching for '${props.query}'`
                }
            />
            {props.packages && props.totalCount ? (
                <SearchResult
                    packages={props.packages}
                    query={props.query}
                    current_path="/search"
                    perPage={props.perPage}
                    page={props.page}
                    totalCount={props.totalCount}
                />
            ) : (
                <Center>
                    <Text>no packages found</Text>
                </Center>
            )}
        </>
    );
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    // JSON.stringify removes undefined fields meaning that the default value processes can be delegated to the API
    // except for query. TODO: However, we have to keep those processes for props.
    const query = context.query.query ?? "";
    const page = context.query.page ? +context.query.page : 1;
    const perPage = context.query.perPage ? +context.query.perPage : PER_PAGE;

    const res = await fetch(
        `${BASE_API_URL}/packages/search?query=${
            context.query.query ?? ""
        }&page=${page}&perPage=${perPage}&sort=${context.query.sort}`,
    );
    const data = await res.json();

    const packages: PackageOverview[] = [];
    for (const rawPkg of data["data"]["results"]) {
        const pkg: PackageOverview = {
            id: rawPkg["id"],
            published_at: rawPkg["published_at"],
            name: rawPkg["name"],
            version: rawPkg["version"],
            edition: rawPkg["edition"],
            description: rawPkg["description"],
        };
        packages.push(pkg);
    }

    return {
        props: {
            packages,
            query,
            perPage,
            page,
            totalCount: data["data"]["total_count"],
        },
    };
};
