import { useUser } from "@supabase/supabase-auth-helpers/react";
import { VStack, Heading, Text, HStack, Spacer, Link, Spinner } from "@chakra-ui/react";
import type { User } from "@supabase/supabase-js";
import { useEffect, useState } from "react";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";
import { ViewIcon } from "@chakra-ui/icons";

import NeedAuth from "~/components/NeedAuth";
import Meta from "~/components/Meta";
import type { Package as PackageType } from "~/utils/types";
import Package from "~/components/Package";

interface DashboardPageProps {
    user: User;
}

function DashboardPage(props: DashboardPageProps): JSX.Element {
    const [loading, setLoading] = useState<boolean>(true);
    const [packages, setPackages] = useState<PackageType[]>([]);

    useEffect(() => {
        setLoading(true);
        supabaseClient
            .rpc<PackageType>("get_uniq_packages", {}, { count: "exact" })
            .select("*") // TODO: Improve selection: name, total downloads, updated_at, ...
            .like("name", `${props.user.user_metadata["user_name"]}/%`)
            .range(0, 5)
            .then(({ data }) => {
                if (data) {
                    setLoading(false);
                    setPackages(data);
                }
            });
    }, [props.user.user_metadata]);

    return (
        <VStack spacing={5}>
            <Heading>Dashboard</Heading>
            <HStack>
                <Text>My packages</Text>
                <Spacer />
                <ViewIcon />
                <Link href={`/packages/${props.user.user_metadata["user_name"]}`}>Show all</Link>
            </HStack>
            {loading ?
                <Spinner /> :
                packages.length > 0 ?
                    <VStack spacing={5}>
                        {packages.map((p) => <Package key={p.id} package={p} group={props.user.user_metadata["user_name"]} />)}
                    </VStack> :
                    <Text>no packages to show</Text>
            }
        </VStack>
    );
}

export default function Dashboard(): JSX.Element {
    const { user } = useUser();

    return (
        <>
            <Meta title="Dashboard" />
            {user ? <DashboardPage user={user} /> : <NeedAuth />}
        </>
    );
}
