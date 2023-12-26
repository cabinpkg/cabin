import { Center, Text } from "@chakra-ui/react";
import type { GetServerSideProps } from "next";

import Meta from "~/components/Meta";
import SearchResult from "~/components/SearchResult";
import { PER_PAGE } from "~/utils/constants";
import { createHasuraClient } from "~/utils/hasuraClient";
import type { PackageOverview } from "~/utils/types";

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

    const hasuraClient = createHasuraClient();
    const data = await hasuraClient.searchPackages({
        name: `%${query}%`,
        limit: perPage,
        offset: (page - 1) * perPage,
    });

    return {
        props: {
            packages: data.packages,
            query,
            perPage,
            page,
            totalCount: data.packages_aggregate?.aggregate?.count,
        },
    };
};
