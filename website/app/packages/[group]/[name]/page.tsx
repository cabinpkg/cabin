import type { Metadata, ResolvingMetadata } from "next";
import { notFound } from "next/navigation";
import { getHasuraClient } from "~/app/_lib/hasuraClient";
import { Pack } from "./_components/pack";

export const revalidate = 86400; // 1 day

type Params = {
    group: string;
    name: string;
};

type Props = {
    params: Params;
    searchParams: { [key: string]: string | string[] | undefined };
};

export async function generateMetadata(
    { params, searchParams }: Props,
    parent: ResolvingMetadata,
): Promise<Metadata> {
    return {
        title: `${params.group}/${params.name} (latest)`,
    };
}

export default async function Name({ params }: { params: Params }) {
    const hasuraClient = getHasuraClient();
    const data = await hasuraClient.getPackagesByName({
        name: `${params.group}/${params.name}`,
    });
    if (!data || data.packages.length === 0) {
        return notFound();
    }

    data.packages.sort((a, b) => {
        const semver = require("semver");
        return semver.rcompare(a.version, b.version);
    });

    return <Pack pack={data.packages[0]} numVersion={data.packages.length} />;
}
