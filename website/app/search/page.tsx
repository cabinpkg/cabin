import { Card, CardBody, CardHeader } from "@heroui/react";
import type { Metadata, ResolvingMetadata } from "next";
import NextLink from "next/link";
import { PER_PAGE } from "../_lib/constants";
import { getHasuraClient } from "../_lib/hasuraClient";
import { Pagination } from "./_components/pagination";

export const revalidate = 86400; // 1 day

type SearchParams = Promise<{ [key: string]: string | string[] | undefined }>;

export async function generateMetadata(
    props: {
        searchParams: SearchParams;
    },
    _parent: ResolvingMetadata,
): Promise<Metadata> {
    const searchParams = await props.searchParams;

    if (!searchParams || searchParams.q === "") {
        return {
            title: "All packages",
        };
    }

    return {
        title: `Search results for "${searchParams.q}"`,
    };
}

export default async function Page(props: { searchParams: SearchParams }) {
    const searchParams = await props.searchParams;

    const query = String(searchParams?.q) ?? "";
    const page = Number(searchParams?.page ?? 1);
    const perPage = Number(searchParams?.perPage ?? PER_PAGE);

    const hasuraClient = getHasuraClient();
    const data = await hasuraClient.searchPackages({
        name: `%${query}%`,
        limit: perPage,
        offset: (page - 1) * perPage,
    });
    if (!data || data.packages.length === 0) {
        return (
            <div className="flex flex-col items-center justify-center h-screen">
                no packages found
            </div>
        );
    }

    const totalCount = data.packages_aggregate?.aggregate?.count ?? 0;
    const currentLast = page * perPage;
    const currentPos = {
        first: currentLast - (perPage - 1),
        last: currentLast > totalCount ? totalCount : currentLast,
    };
    const numPages = Math.ceil(totalCount / perPage);

    const header = (
        <span>
            Displaying&nbsp;
            <span className="font-bold">
                {currentPos.first}-{currentPos.last}
            </span>
            &nbsp;of&nbsp;<span className="font-bold">{totalCount}</span>
            &nbsp;total results
        </span>
    );

    return (
        <div className="flex flex-col items-center justify-center m-4 gap-4">
            {header}
            {data.packages.map((pkg) => (
                <Card
                    key={pkg.id}
                    id={pkg.id}
                    className="w-full max-w-[500px] p-4"
                    as={NextLink}
                    href={`/packages/${pkg.name}/${pkg.version}`}
                >
                    <CardHeader className="justify-between">
                        <p className="font-bold">{pkg.name}</p>
                        <p className="text-gray-500">{pkg.version}</p>
                    </CardHeader>
                    <CardBody>
                        <p className="text-gray-500">{pkg.description}</p>
                    </CardBody>
                </Card>
            ))}
            <Pagination
                query={query}
                page={page}
                numPages={numPages}
                perPage={perPage}
            />
        </div>
    );
}
