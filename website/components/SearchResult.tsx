import { Center, HStack, Select, Spacer, Text, VStack } from "@chakra-ui/react";
import { faListOl } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { ChangeEvent } from "react";
import { useCallback, useState } from "react";

import Package from "~/components/Package";
import SearchPagination from "~/components/SearchPagination";
import type { PackageOverview, Position } from "~/utils/types";

const perPageSelections = [5, 10, 30, 50, 100] as const;

interface SearchResultProps {
    packages: PackageOverview[];
    query?: string;
    current_path: string;
    perPage: number;
    page: number;
    totalCount: number;
}

export default function SearchResult(props: SearchResultProps): JSX.Element {
    const [currentPos, setCurrentPos] = useState<Position>({
        first: 0,
        last: 0,
    });
    const [perPage, setPerPage] = useState<number>(props.perPage);

    const handlePerPageChange = useCallback(
        (event: ChangeEvent<HTMLSelectElement>) => {
            setPerPage(parseInt(event.target.value));
        },
        [],
    );

    return (
        <Center>
            <VStack spacing={5}>
                <HStack>
                    {props.totalCount !== 0 ? (
                        <Text fontSize="sm">
                            Displaying{" "}
                            <Text as="b">
                                {currentPos.first}-{currentPos.last}
                            </Text>{" "}
                            of <Text as="b">{props.totalCount}</Text> total
                            results
                        </Text>
                    ) : (
                        <Text as="b">0 packages found.</Text>
                    )}
                    <Spacer />
                    <FontAwesomeIcon icon={faListOl} width={20} />
                    <Select
                        width={79}
                        value={perPage}
                        onChange={handlePerPageChange}
                    >
                        {perPageSelections.map((v) => (
                            <option key={v} value={v}>
                                {v}
                            </option>
                        ))}
                    </Select>
                </HStack>
                <VStack spacing={5}>
                    {props.packages.map((p) => (
                        <Package key={p.id} package={p} />
                    ))}
                </VStack>
                <SearchPagination
                    pathname={props.current_path}
                    query={props.query ? { query: props.query } : undefined}
                    setCurrentPos={setCurrentPos}
                    perPage={perPage}
                    page={props.page}
                    totalCount={props.totalCount}
                />
            </VStack>
        </Center>
    );
}
