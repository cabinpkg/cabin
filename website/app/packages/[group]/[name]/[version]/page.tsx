import type { Metadata, ResolvingMetadata } from "next";
import { notFound } from "next/navigation";
import { getHasuraClient } from "~/app/_lib/hasuraClient";
import { Pack } from "../_components/pack";

export const revalidate = 86400; // 1 day

type Params = Promise<{
    group: string;
    name: string;
    version: string;
}>;

export async function generateMetadata(
    props: {
        params: Params;
    },
    _parent: ResolvingMetadata,
): Promise<Metadata> {
    const params = await props.params;
    return {
        title: `${params.group}/${params.name} (v${params.version})`,
    };
}

export default async function Page(props: { params: Params }) {
    const params = await props.params;

    const hasuraClient = getHasuraClient();
    const data = await hasuraClient.getPackageByNameAndVersion({
        name: `${params.group}/${params.name}`,
        version: params.version,
    });
    if (!data || data.packages.length === 0) {
        return notFound();
    }

    return (
        <Pack
            pack={data.packages[0]}
            numVersion={data.packages_aggregate?.aggregate?.count ?? 0}
        />
    );
}
