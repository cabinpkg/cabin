import type { GetStaticPaths, GetStaticProps } from "next";

import Meta from "~/components/Meta";
import PackageDetails from "~/components/PackageDetails";
import { createHasuraClient } from "~/utils/hasuraClient";
import type { Package } from "~/utils/types";

interface VersionProps {
    package: Package;
    numVersions: number;
}

export default function Version(props: VersionProps): JSX.Element {
    return (
        <>
            <Meta
                title={`${props.package.name} (v${props.package.version})`}
                package={props.package}
                description={props.package.description}
            />
            <PackageDetails
                package={props.package}
                numVersions={props.numVersions}
            />
        </>
    );
}

export const getStaticProps: GetStaticProps = async (context) => {
    const group = context.params?.group;
    const name = context.params?.name;
    const version = context.params?.version;
    if (
        typeof group !== "string" ||
        typeof name !== "string" ||
        typeof version !== "string"
    ) {
        return {
            notFound: true,
        };
    }

    const hasuraClient = createHasuraClient();
    const data = await hasuraClient.getPackageByNameAndVersion({
        name: `${group}/${name}`,
        version,
    });
    if (!data || data.packages.length === 0) {
        return {
            notFound: true,
        };
    }

    return {
        props: {
            package: data.packages[0],
            numVersions: data.packages_aggregate?.aggregate?.count,
        },
        revalidate: 86400, // one day; version specific page should not be updated frequently
    };
};

export const getStaticPaths: GetStaticPaths = async () => {
    return {
        paths: [],
        fallback: "blocking",
    };
};
