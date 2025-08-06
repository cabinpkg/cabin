import {
    faBox,
    faCube,
    faSearch,
    faTag,
} from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Card, CardBody, CardHeader, Chip, Divider } from "@heroui/react";
import type { Metadata, ResolvingMetadata } from "next";
import NextLink from "next/link";
import { format } from "timeago.js";
import { GradientIcon } from "../_components/gradient-icon";
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

    if (!searchParams || !searchParams.q || searchParams.q === "") {
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

    const query = searchParams?.q ? String(searchParams.q) : "";
    const page = Number(searchParams?.page ?? 1);
    const perPage = Number(searchParams?.perPage ?? PER_PAGE);

    const hasuraClient = getHasuraClient();
    const data = await hasuraClient.searchPackages({
        name: `%${query}%`,
        limit: perPage,
        offset: (page - 1) * perPage,
    });

    const totalCount = data?.packages_aggregate?.aggregate?.count ?? 0;
    const currentLast = page * perPage;
    const currentPos = {
        first: currentLast - (perPage - 1),
        last: currentLast > totalCount ? totalCount : currentLast,
    };
    const numPages = Math.ceil(totalCount / perPage);

    // Empty state
    if (!data || data.packages.length === 0) {
        return (
            <div className="container mx-auto max-w-4xl px-6 py-12">
                {/* Header */}
                <div className="text-center mb-8">
                    <div className="flex items-center justify-center gap-3 mb-4">
                        <GradientIcon icon={faSearch} size="lg" />
                        <h1 className="text-3xl md:text-4xl font-bold">
                            {query
                                ? `Search results for "${query}"`
                                : "All packages"}
                        </h1>
                    </div>
                </div>

                {/* Empty state */}
                <div className="text-center py-16">
                    <FontAwesomeIcon
                        icon={faBox}
                        className="text-default-300 text-6xl mb-6"
                    />
                    <h2 className="text-2xl font-semibold mb-3">
                        No packages found
                    </h2>
                    <p className="text-default-500 text-lg">
                        {query
                            ? `We couldn't find any packages matching "${query}". Try a different search term.`
                            : "No packages are available at the moment."}
                    </p>
                </div>
            </div>
        );
    }

    return (
        <div className="container mx-auto max-w-4xl px-6 py-8">
            {/* Header */}
            <div className="mb-8">
                <div className="flex items-center gap-3 mb-4">
                    <GradientIcon icon={faSearch} size="lg" />
                    <div>
                        <h1 className="text-3xl md:text-4xl font-bold">
                            {query
                                ? `Search results for "${query}"`
                                : "All packages"}
                        </h1>
                        <p className="text-default-600 mt-1">
                            Displaying{" "}
                            <span className="font-semibold text-primary">
                                {currentPos.first}-{currentPos.last}
                            </span>{" "}
                            of{" "}
                            <span className="font-semibold text-primary">
                                {totalCount}
                            </span>{" "}
                            packages
                        </p>
                    </div>
                </div>
            </div>

            {/* Results */}
            <div className="space-y-4 mb-8">
                {data.packages.map((pkg) => (
                    <Card
                        key={pkg.id}
                        className="w-full hover:shadow-lg transition-shadow duration-200 hover:scale-[1.01]"
                        isPressable
                        as={NextLink}
                        href={`/packages/${pkg.name}/${pkg.version}`}
                    >
                        <CardHeader className="pb-3">
                            <div className="flex items-start justify-between w-full">
                                <div className="flex items-center gap-3 flex-1">
                                    <div className="flex items-center justify-center w-10 h-10 bg-gradient-to-br from-primary-100 to-secondary-100 rounded-lg">
                                        <FontAwesomeIcon
                                            icon={faCube}
                                            className="text-primary text-sm"
                                        />
                                    </div>
                                    <div className="flex-1 min-w-0">
                                        <h3 className="text-lg font-semibold text-foreground truncate">
                                            {pkg.name}
                                        </h3>
                                        <div className="flex items-center gap-2 mt-1">
                                            <FontAwesomeIcon
                                                icon={faTag}
                                                className="text-default-400 text-xs"
                                            />
                                            <span className="text-sm text-default-500">
                                                v{pkg.version}
                                            </span>
                                        </div>
                                    </div>
                                </div>
                                <div className="flex flex-col items-end gap-2 shrink-0 ml-4">
                                    <Chip
                                        size="sm"
                                        variant="flat"
                                        color="primary"
                                    >
                                        C++{pkg.edition.toString().slice(-2)}
                                    </Chip>
                                    <span className="text-xs text-default-400">
                                        {format(pkg.published_at)}
                                    </span>
                                </div>
                            </div>
                        </CardHeader>
                        {pkg.description && (
                            <>
                                <Divider />
                                <CardBody className="pt-3">
                                    <p className="text-default-600 line-clamp-2">
                                        {pkg.description}
                                    </p>
                                </CardBody>
                            </>
                        )}
                    </Card>
                ))}
            </div>

            {/* Pagination */}
            {numPages > 1 && (
                <div className="flex justify-center">
                    <Pagination
                        query={query}
                        page={page}
                        numPages={numPages}
                        perPage={perPage}
                    />
                </div>
            )}
        </div>
    );
}
