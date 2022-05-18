import type { GetServerSideProps } from "next";
import { supabaseServerClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package as PackageType } from "~/utils/types";
import PackageDetails from "~/components/PackageDetails";
import Meta from "~/components/Meta";

interface VersionProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

export default function Version(props: VersionProps): JSX.Element {
    return (
        <>
            <Meta title={`${props.package.name}: ${props.package.version}`} />
            <PackageDetails package={props.package} versions={props.versions} dependents={props.dependents} />
        </>
    );
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const group = context.query.group;
    const name = context.query.name;
    const version = context.query.version;

    const { data: packages, error: e1 } = await supabaseServerClient(context)
        .rpc<PackageType>("get_packages")
        .select("*") // TODO: Improve selection: name, total downloads, updated_at, ...
        .eq("name", `${group}/${name}`);
    if (e1) {
        console.error(e1);
    }

    if (packages && packages.length > 0) {
        const specificPackage = packages.find((p) => p.version === version);
        if (specificPackage) {
            // Retrieve dependents
            const { data: dependents, error: e2 } = await supabaseServerClient(context)
                .rpc<PackageType>("get_dependents", { "depname": specificPackage.name })
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
            };
        }
    }
    return {
        notFound: true,
    };
};
