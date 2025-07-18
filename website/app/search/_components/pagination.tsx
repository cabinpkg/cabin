"use client";

import { Pagination as NextUIPagination } from "@heroui/react";
import { useRouter } from "next/navigation";

type Props = {
    query: string;
    page: number;
    numPages: number;
    perPage: number;
};

export function Pagination({ query, page, numPages, perPage }: Props) {
    const router = useRouter();

    const handlePageChange = (page: number) => {
        router.push(`/search?q=${query}&page=${page}&perPage=${perPage}`);
    };

    return (
        <NextUIPagination
            showControls
            total={numPages}
            initialPage={page}
            onChange={handlePageChange}
        />
    );
}
