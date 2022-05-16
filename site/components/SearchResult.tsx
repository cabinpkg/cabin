import type { ChangeEvent } from "react";
import { useCallback, useState } from "react";
import { Center, HStack, Select, Text, VStack } from "@chakra-ui/react";

import type { Package as PackageType, Position } from "~/utils/types";
import SearchPagination from "~/components/SearchPagination";
import Package from "~/components/Package";

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
    const handleChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
        setPerPage(parseInt(event.target.value));
    }, []);

    return (
        <Center>
            <VStack spacing={5}>
                <HStack spacing={10}>
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
                    <Select width={79} value={perPage} onChange={handleChange}>
                        <option value="5">5</option>
                        <option value="10">10</option>
                        <option value="30">30</option>
                        <option value="50">50</option>
                        <option value="100">100</option>
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
                    page={props.page}
                    totalCount={props.totalCount}
                />
            </VStack>
        </Center>
    );
}
