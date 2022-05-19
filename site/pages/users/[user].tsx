import type { GetServerSideProps } from "next";
import { supabaseServerClient } from "@supabase/supabase-auth-helpers/nextjs";
import { VStack, Text } from "@chakra-ui/react";

import type { Package } from "~/utils/types";
import { PER_PAGE } from "~/utils/constants";
import SearchResult from "~/components/SearchResult";
import Meta from "~/components/Meta";

interface GroupProps {
    packages: Package[];
    user: string;
    perPage: number;
    page: number;
    totalCount: number;
}

export default function Group(props: GroupProps): JSX.Element {
    return (
        <>
            <Meta title={props.user} />
            <VStack>
                <Text>Packages owned by <Text as="b">{props.user}</Text></Text>
                <SearchResult
                    packages={props.packages}
                    current_path={`/users/${props.user}`}
                    perPage={props.perPage}
                    page={props.page}
                    totalCount={props.totalCount}
                />
            </VStack>
        </>
    );
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const user = context.query.user;
    const page = context.query.page ? +context.query.page : 1;
    const perPage = context.query.perPage ? +context.query.perPage : PER_PAGE;

    let request = supabaseServerClient(context)
        .rpc<Package>("get_owned_packages", { username: user }, { count: "exact" })
        .select("*"); // TODO: Improve selection: name, total downloads, updated_at, ...

    const startIndex = (page - 1) * perPage;
    request = request.range(startIndex, startIndex + (perPage - 1));

    const { data, count, error } = await request;
    if (error) {
        console.error(error);
    }
    if (data && count) {
        return {
            props: {
                packages: data,
                user,
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
