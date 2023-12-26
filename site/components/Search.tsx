import { SearchIcon } from "@chakra-ui/icons";
import {
    Container,
    Input,
    InputGroup,
    InputRightElement,
} from "@chakra-ui/react";
import { useRouter } from "next/router";
import type { ChangeEvent, KeyboardEvent } from "react";
import { useCallback, useEffect, useState } from "react";

export default function Search(): JSX.Element {
    const router = useRouter();

    const [query, setQuery] = useState<string>("");
    const updateInputValue = useCallback(
        (event: ChangeEvent<HTMLInputElement>) => {
            setQuery(event.target.value);
        },
        [],
    );

    const searchOnEnter = useCallback(
        async (e: KeyboardEvent<HTMLInputElement>): Promise<void> => {
            if (query.length !== 0 && e.key === "Enter") {
                e.preventDefault();
                await router.push({
                    pathname: "/search",
                    query: { query },
                });
            }
        },
        [query, router],
    );

    useEffect(() => {
        if (
            router.pathname === "/search" &&
            router.query.query &&
            typeof router.query.query === "string"
        ) {
            setQuery(router.query.query);
        }
    }, [router.pathname, router.query.query]);

    return (
        <Container>
            <InputGroup>
                <Input
                    type="search"
                    placeholder="Search packages ..."
                    value={query}
                    onChange={updateInputValue}
                    onKeyDown={searchOnEnter}
                />
                <InputRightElement>
                    <SearchIcon />
                </InputRightElement>
            </InputGroup>
        </Container>
    );
}
