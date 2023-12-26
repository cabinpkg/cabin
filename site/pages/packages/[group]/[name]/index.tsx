import type { GetStaticPaths, GetStaticProps } from "next";

import Meta from "~/components/Meta";
import PackageDetails from "~/components/PackageDetails";
import { createHasuraClient } from "~/utils/hasuraClient";
import type { Package } from "~/utils/types";

interface NameProps {
    package: Package;
    numVersions: number;
}

export default function Name(props: NameProps): JSX.Element {
    return (
        <>
            <Meta
                title={`${props.package.name} (latest)`}
                package={{ name: props.package.name, version: "latest" }}
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
    if (typeof group !== "string" || typeof name !== "string") {
        return {
            notFound: true,
        };
    }

    const hasuraClient = createHasuraClient();
    const data = await hasuraClient.getPackagesByName({
        name: `${group}/${name}`,
    });
    if (!data || data.packages.length === 0) {
        return {
            notFound: true,
        };
    }

    data.packages.sort((a, b) => {
        const semver = require("semver");
        return semver.rcompare(a.version, b.version);
    });

    return {
        props: {
            package: data.packages[0],
            numVersions: data.packages.length,
        },
        revalidate: 86400, // one day; name specific page should not be updated frequently
    };
};

export const getStaticPaths: GetStaticPaths = async () => {
    return {
        paths: [],
        fallback: "blocking",
    };
};
