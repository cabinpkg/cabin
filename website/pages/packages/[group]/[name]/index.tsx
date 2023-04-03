import type { GetStaticProps, GetStaticPaths } from "next";

import type { Package } from "~/utils/types";
import PackageDetails from "~/components/PackageDetails";
import Meta from "~/components/Meta";
import { BASE_API_URL } from "~/utils/constants";

interface NameProps {
    package: Package;
    versions: string[];
    dependents: Package[];
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
                versions={props.versions}
                dependents={props.dependents}
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

    const res = await fetch(
        `${BASE_API_URL}/packages/${group}/${name}/details`,
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
        const latestPackage = packages[0];
        // Retrieve dependents
        const res = await fetch(
            `${BASE_API_URL}/packages/${latestPackage.name}/dependents`,
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
                package: latestPackage,
                versions: packages.map((p) => p.version),
                dependents,
            },
            revalidate: 60,
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
