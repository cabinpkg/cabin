import type { GetServerSideProps } from "next";
import { VStack, Text, Center } from "@chakra-ui/react";
import { supabaseServerClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package as PackageType } from "../../../types";
import Package from "../../../components/Package";

interface GroupProps {
    packages: PackageType[];
    group: string;
    totalCount: number;
}

export default function Group(props: GroupProps): JSX.Element {
    return (
        <Center>
            <VStack maxWidth={700} align="left" spacing={5}>
                <Text>Packages owned by <Text as="b">{props.group}</Text></Text>
                {props.totalCount !== 0 ?
                    <Text size="xs">
                        {/* TODO: Implement this logic correctly and pagination */}
                        Displaying <Text as="b">1-{props.totalCount}</Text> of <Text as="b">{props.totalCount}</Text> total results
                    </Text> :
                    <Text as="b">
                        0 packages found.
                    </Text>
                }
                <VStack spacing={5}>
                    {props.packages.map((p) => <Package key={p.id} package={p} group={props.group} />)}
                </VStack>
            </VStack>
        </Center>
    );
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const group = context.query.group;

    const { data, count } = await supabaseServerClient(context)
        .rpc<PackageType>("get_uniq_packages", {}, { count: "exact" })
        .select("*") // TODO: Improve selection: name, total downloads, updated_at, ...
        .like("name", `${group}/%`);

    if (data && count) {
        return {
            props: {
                packages: data,
                group,
                totalCount: count,
            },
        };
    }
    return {
        notFound: true,
    };
};
