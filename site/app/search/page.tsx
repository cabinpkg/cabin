import { Card, CardBody, CardHeader } from "@nextui-org/react";
import type { Metadata, ResolvingMetadata } from "next";
import NextLink from "next/link";
import { notFound } from "next/navigation";
import { PER_PAGE } from "../_lib/constants";
import { getHasuraClient } from "../_lib/hasuraClient";
import { Pagination } from "./_components/pagination";

type Props = {
    searchParams: { [key: string]: string | string[] | undefined };
};

export async function generateMetadata(
    { searchParams }: Props,
    parent: ResolvingMetadata,
): Promise<Metadata> {
    if (!searchParams || searchParams.q === "") {
        return {
            title: "All packages",
        };
    }

    return {
        title: `Search results for "${searchParams.q}"`,
    };
}

export default async function Search({ searchParams }: Props) {
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
        return notFound();
    }

    const totalCount = data.packages_aggregate?.aggregate?.count ?? 0;
    const currentLast = page * perPage;
    const currentPos = {
        first: currentLast - (perPage - 1),
        last: currentLast > totalCount ? totalCount : currentLast,
    };
    const numPages = Math.ceil(totalCount / perPage);

    if (totalCount === 0) {
        return (
            <div className="flex flex-col items-center justify-center h-screen">
                no packages found
            </div>
        );
    }

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
