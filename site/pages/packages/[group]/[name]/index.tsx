import type { GetStaticProps, GetStaticPaths } from "next";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package } from "~/utils/types";
import PackageDetails from "~/components/PackageDetails";
import Meta from "~/components/Meta";

interface NameProps {
    package: Package;
    versions: string[];
    dependents: Package[];
}

export default function Name(props: NameProps): JSX.Element {
    return (
        <>
            <Meta title={`${props.package.name}: latest`} />
            <PackageDetails package={props.package} versions={props.versions} dependents={props.dependents} />
        </>
    );
}

export const getStaticProps: GetStaticProps = async (context) => {
    const group = context.params?.group;
    const name = context.params?.name;
    if (typeof group !== "string" ||
        typeof name !== "string") {
        return {
            notFound: true,
        };
    }

    const { data: packages, error: e1 } = await supabaseClient
        .rpc<Package>("get_packages")
        .select("*")
        .eq("name", `${group}/${name}`);
    if (e1) {
        console.error(e1);
    }

    if (packages && packages.length > 0) {
        const latestPackage = packages[0];
        // Retrieve dependents
        const { data: dependents, error: e2 } = await supabaseClient
            .rpc<Package>("get_dependents", { "depname": latestPackage.name })
            .select("*");
        if (e2) {
            console.error(e2);
        }

        return {
            props: {
                package: latestPackage,
                versions: packages.map((p) => p.version),
                dependents,
            },
            revalidate: 10,
        };
    }
    return {
        notFound: true,
    };
};

export const getStaticPaths: GetStaticPaths = async () => {
    return {
        paths: [],
        fallback: "blocking",
    };
};
