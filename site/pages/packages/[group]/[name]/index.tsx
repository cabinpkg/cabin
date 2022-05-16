import type { GetServerSideProps } from "next";
import { supabaseServerClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package as PackageType } from "~/utils/types";
import PackageDetails from "~/components/PackageDetails";

interface NameProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

export default function Name(props: NameProps): JSX.Element {
    return <PackageDetails package={props.package} versions={props.versions} dependents={props.dependents} />;
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const group = context.query.group;
    const name = context.query.name;

    const { data: packages } = await supabaseServerClient(context)
        .rpc<PackageType>("get_packages")
        .select("*") // TODO: Improve selection: name, total downloads, updated_at, ...
        .eq("name", `${group}/${name}`);

    if (packages && packages.length > 0) {
        const latestPackage = packages[0];
        // Retrieve dependents
        const { data: dependents } = await supabaseServerClient(context)
            .rpc<PackageType>("get_dependents", { "depname": latestPackage.name })
            .select("*");

        return {
            props: {
                package: latestPackage,
                versions: packages.map((p) => p.version),
                dependents,
            },
        };
    }
    return {
        notFound: true,
    };
};
