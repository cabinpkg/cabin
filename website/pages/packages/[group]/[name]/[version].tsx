import type { GetStaticProps , GetStaticPaths } from "next";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package } from "~/utils/types";
import PackageDetails from "~/components/PackageDetails";
import Meta from "~/components/Meta";

interface VersionProps {
    package: Package;
    versions: string[];
    dependents: Package[];
}

export default function Version(props: VersionProps): JSX.Element {
    return (
        <>
            <Meta
                title={`${props.package.name} (v${props.package.version})`}
                package={props.package}
                description={props.package.description}
            />
            <PackageDetails package={props.package} versions={props.versions} dependents={props.dependents} />
        </>
    );
}

export const getStaticProps: GetStaticProps = async (context) => {
    const group = context.params?.group;
    const name = context.params?.name;
    const version = context.params?.version;
    if (typeof group !== "string" ||
        typeof name !== "string" ||
        typeof version !== "string") {
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
        const specificPackage = packages.find((p) => p.version === version);
        if (specificPackage) {
            // Retrieve dependents
            const { data: dependents, error: e2 } = await supabaseClient
                .rpc<Package>("get_dependents", { "depname": specificPackage.name })
                .select("*");
            if (e2) {
                console.error(e2);
            }

            return {
                props: {
                    package: specificPackage,
                    versions: packages.map((p) => p.version),
                    dependents,
                },
                revalidate: 86400, // one day; version specific page should not be updated frequently
            };
        }
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
