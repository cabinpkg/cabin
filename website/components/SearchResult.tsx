import type { ChangeEvent } from "react";
import { useCallback, useState } from "react";
import { Center, HStack, Select, Spacer, Text, VStack } from "@chakra-ui/react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faSort, faListOl } from "@fortawesome/free-solid-svg-icons";
import humanizeString from "humanize-string";

import type { Package as PackageType, Position } from "~/utils/types";
import SearchPagination from "~/components/SearchPagination";
import Package from "~/components/Package";

const perPageSelections = [5, 10, 30, 50, 100] as const;
const sortSelections = ["relevance", "newlyPublished"] as const;
export type Sort = typeof sortSelections[number];

interface SearchResultProps {
    packages: PackageType[];
    group?: string;
    query?: string;
    pathname: string;
    perPage: number;
    page: number;
    totalCount: number;
}

export default function SearchResult(props: SearchResultProps): JSX.Element {
    const [currentPos, setCurrentPos] = useState<Position>({ first: 0, last: 0 });
    const [perPage, setPerPage] = useState<number>(props.perPage);
    const [sort, setSort] = useState<Sort>("relevance");

    const handlePerPageChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
        setPerPage(parseInt(event.target.value));
    }, []);
    const handleSortChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
        setSort(event.target.value as Sort);
    }, []);

    return (
        <Center>
            <VStack spacing={5}>
                <HStack>
                    {props.totalCount !== 0 ?
                        <Text size="xs">
                            Displaying <Text as="b">
                                {currentPos.first}-{currentPos.last}
                            </Text> of <Text as="b">
                                {props.totalCount}
                            </Text> total results
                        </Text> :
                        <Text as="b">
                            0 packages found.
                        </Text>
                    }
                    <Spacer />
                    <FontAwesomeIcon icon={faListOl} />
                    <Select width={79} value={perPage} onChange={handlePerPageChange}>
                        {perPageSelections.map((v) => <option key={v} value={v}>{v}</option>)}
                    </Select>
                    <FontAwesomeIcon icon={faSort} />
                    <Select width={200} value={sort} onChange={handleSortChange}>
                        {sortSelections.map((v) => <option key={v} value={v}>{humanizeString(v)}</option>)}
                    </Select>
                </HStack>
                <VStack spacing={5}>
                    {props.packages.map((p) => <Package key={p.id} package={p} group={props.group} />)}
                </VStack>
                <SearchPagination
                    pathname={props.pathname}
                    query={props.query ? { query: props.query } : undefined}
                    setCurrentPos={setCurrentPos}
                    perPage={perPage}
                    sort={sort}
                    page={props.page}
                    totalCount={props.totalCount}
                />
            </VStack>
        </Center>
    );
}
