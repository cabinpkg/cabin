import type { GetStaticProps, GetStaticPaths } from "next";

import type { Package } from "~/utils/types";
import PackageDetails from "~/components/PackageDetails";
import Meta from "~/components/Meta";
import { BASE_API_URL } from "~/utils/constants";

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
            <PackageDetails
                package={props.package}
                versions={props.versions}
                dependents={props.dependents}
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

    const res = await fetch(
        `${BASE_API_URL}/packages/${group}/${name}/specific`,
    );
    const data = await res.json();

    const packages: Package[] = [];
    for (const rawPkg of data["data"]) {
        const pkg: Package = {
            id: rawPkg["id"],
            published_at: rawPkg["published_at"],
            name: rawPkg["name"],
            version: rawPkg["version"],
            description: rawPkg["description"],
            edition: rawPkg["edition"],
            authors: rawPkg["authors"],
            repository: rawPkg["repository"],
            license: rawPkg["license"],
            metadata: rawPkg["metadata"],
            readme: rawPkg["readme"],
        };
        packages.push(pkg);
    }

    if (packages && packages.length > 0) {
        const specificPackage = packages.find((p) => p.version === version);
        if (specificPackage) {
            // Retrieve dependents
            const res = await fetch(
                `${BASE_API_URL}/packages/${specificPackage.name}/dependents`,
            );
            const data = await res.json();

            const dependents: Package[] = [];
            for (const rawPkg of data["data"]) {
                const pkg: Package = {
                    id: rawPkg["id"],
                    published_at: rawPkg["published_at"],
                    name: rawPkg["name"],
                    version: rawPkg["version"],
                    description: rawPkg["description"],
                    edition: rawPkg["edition"],
                    authors: rawPkg["authors"],
                    repository: rawPkg["repository"],
                    license: rawPkg["license"],
                    metadata: rawPkg["metadata"],
                    readme: rawPkg["readme"],
                };
                dependents.push(pkg);
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
