import type { GetServerSideProps } from "next";
import { supabaseServerClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package as PackageType } from "~/utils/types";
import PackageDetails from "~/components/PackageDetails";

interface VersionProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

export default function Version(props: VersionProps): JSX.Element {
    return <PackageDetails package={props.package} versions={props.versions} dependents={props.dependents} />;
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const group = context.query.group;
    const name = context.query.name;
    const version = context.query.version;

    const { data: packages } = await supabaseServerClient(context)
        .rpc<PackageType>("get_packages")
        .select("*") // TODO: Improve selection: name, total downloads, updated_at, ...
        .eq("name", `${group}/${name}`);

    if (packages && packages.length > 0) {
        const specificPackage = packages.find((p) => p.version === version);
        if (specificPackage) {
            // Retrieve dependents
            const { data: dependents } = await supabaseServerClient(context)
                .rpc<PackageType>("get_dependents", { "depname": specificPackage.name })
                .select("*");

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
