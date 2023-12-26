"use client";

// TODO: metadata

import {
    Pagination,
    Spacer,
    Spinner,
    Table,
    TableBody,
    TableCell,
    TableColumn,
    TableHeader,
    TableRow,
} from "@nextui-org/react";
import NextLink from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { useEffect, useState } from "react";
import { SearchPackagesQuery } from "~/graphql";
import { PER_PAGE } from "../_lib/constants";

export default function Search() {
    const router = useRouter();

    const searchParams = useSearchParams();
    const query = searchParams?.get("q") ?? "";
    const page = Number(searchParams?.get("page") ?? 1);
    const perPage = Number(searchParams?.get("perPage") ?? PER_PAGE);
    const [totalCount, setTotalCount] = useState(0);

    const currentLast = page * perPage;
    const currentPos = {
        first: currentLast - (perPage - 1),
        last: currentLast > totalCount ? totalCount : currentLast,
    };
    const numPages = Math.ceil(totalCount / perPage);

    const [loading, setLoading] = useState(true);
    const [packages, setPackages] = useState<SearchPackagesQuery["packages"]>(
        [],
    );

    useEffect(() => {
        setLoading(true);
        fetch(`/search/api?q=${query}&page=${page}&perPage=${perPage}`)
            .then((response) => response.json() as Promise<SearchPackagesQuery>)
            .then((data: SearchPackagesQuery) => {
                if (!data) {
                    return;
                }
                setPackages(data.packages);
                setTotalCount(data.packages_aggregate?.aggregate?.count ?? 0);
                setLoading(false);
            })
            .catch(() => {
                setLoading(false);
            });
    }, [query, page, perPage]);

    if (loading) {
        return (
            <div className="flex flex-col items-center justify-center h-screen">
                <Spinner size={"lg"} />
            </div>
        );
    }

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

    const handlePageChange = (page: number) => {
        router.push(`/search?q=${query}&page=${page}&perPage=${perPage}`);
    };

    return (
        <div className="flex flex-col items-center justify-center m-4">
            {header}
            <Spacer y={4} />
            <Table
                removeWrapper
                isStriped
                selectionMode="single"
                aria-label="Search results"
            >
                <TableHeader>
                    <TableColumn>NAME</TableColumn>
                    <TableColumn>VERSION</TableColumn>
                    <TableColumn>EDITION</TableColumn>
                </TableHeader>
                <TableBody emptyContent={"No packages to display."}>
                    {packages.map((pkg) => (
                        <TableRow
                            key={pkg.id}
                            as={NextLink}
                            href={`/packages/${pkg.name}/${pkg.version}`}
                            className="hover:cursor-pointer"
                        >
                            <TableCell>{pkg.name}</TableCell>
                            <TableCell>{pkg.version}</TableCell>
                            <TableCell>{pkg.edition}</TableCell>
                        </TableRow>
                    ))}
                </TableBody>
            </Table>
            <Spacer y={4} />
            <Pagination
                showControls
                total={numPages}
                initialPage={page}
                onChange={handlePageChange}
            />
        </div>
    );
}
