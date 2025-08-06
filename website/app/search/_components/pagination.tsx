"use client";

import { Pagination as NextUIPagination } from "@heroui/react";
import { useRouter } from "next/navigation";
import { useTransition } from "react";

type Props = {
    query: string;
    page: number;
    numPages: number;
    perPage: number;
};

export function Pagination({ query, page, numPages, perPage }: Props) {
    const router = useRouter();
    const [isPending, startTransition] = useTransition();

    const handlePageChange = (page: number) => {
        startTransition(() => {
            router.push(
                `/search?q=${encodeURIComponent(query)}&page=${page}&perPage=${perPage}`,
            );
            // Scroll to top smoothly after navigation
            setTimeout(() => {
                window.scrollTo({ top: 0, behavior: "smooth" });
            }, 100);
        });
    };

    return (
        <NextUIPagination
            showControls
            total={numPages}
            initialPage={page}
            onChange={handlePageChange}
            isDisabled={isPending}
            classNames={{
                wrapper: isPending ? "opacity-50" : "",
            }}
        />
    );
}
