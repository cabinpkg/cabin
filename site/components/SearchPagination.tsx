import {
    Pagination,
    PaginationContainer, PaginationNext,
    PaginationPage,
    PaginationPageGroup,
    PaginationPrevious, usePagination,
} from "@ajna/pagination";
import { ChevronLeftIcon, ChevronRightIcon } from "@chakra-ui/icons";
import type { Dispatch, SetStateAction } from "react";
import { useEffect } from "react";
import { useRouter } from "next/router";

import type { Position } from "~/utils/types";

interface SearchPaginationProps {
    pathname: string;
    query?: Record<string, unknown>;
    setCurrentPos: Dispatch<SetStateAction<Position>>;
    perPage: number;
    page: number;
    totalCount: number;
}

export default function SearchPagination(props: SearchPaginationProps): JSX.Element {
    const router = useRouter();
    const {
        currentPage,
        setCurrentPage,
        pagesCount,
        pages,
        pageSize,
        setPageSize,
    } = usePagination({
        total: props.totalCount,
        initialState: {
            pageSize: props.perPage,
            currentPage: props.page,
        },
    });

    useEffect(() => {
        router.push({
            pathname: props.pathname,
            query: {
                page: currentPage,
                perPage: pageSize,
                ...props.query,
            },
        });
    }, [currentPage, pageSize]);

    useEffect(() => {
        const currentLast = currentPage * pageSize;
        props.setCurrentPos({
            first: currentLast - (pageSize - 1),
            last: currentLast > props.totalCount ? props.totalCount : currentLast,
        });
    }, [currentPage, props.totalCount, pageSize]);

    useEffect(() => {
        setPageSize(props.perPage);
        setCurrentPage(1); // Initialize the current page
    }, [props.perPage, setCurrentPage, setPageSize]);

    // eslint-disable-next-line @typescript-eslint/ban-ts-comment
    return ( // @ts-ignore
        <Pagination
            pagesCount={pagesCount}
            currentPage={currentPage}
            onPageChange={setCurrentPage}
        >
            <PaginationContainer>
                <PaginationPrevious marginRight={5} paddingX={3}>
                    <ChevronLeftIcon />
                </PaginationPrevious>
                <PaginationPageGroup>
                    {pages.map((page: number) => (
                        <PaginationPage
                            key={`pagination_page_${page}`}
                            page={page}
                            fontSize="sm"
                            paddingX={3}
                            isDisabled={page === currentPage}
                        />
                    ))}
                </PaginationPageGroup>
                <PaginationNext marginLeft={5} paddingX={3}>
                    <ChevronRightIcon />
                </PaginationNext>
            </PaginationContainer>
        </Pagination>
    );
}
