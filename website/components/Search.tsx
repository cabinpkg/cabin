import { Container, Input, InputGroup, InputRightElement } from "@chakra-ui/react";
import { SearchIcon } from "@chakra-ui/icons";
import type { ChangeEvent } from "react";
import { useCallback, useEffect, useState } from "react";
import { useRouter } from "next/router";

export default function Search(): JSX.Element {
    const router = useRouter();

    const [query, setQuery] = useState<string>("");
    const updateInputValue = useCallback((event: ChangeEvent<HTMLInputElement>) => {
        setQuery(event.target.value);
    }, []);

    useEffect(() => {
        if (router.pathname == "/search" && router.query.query && typeof router.query.query === "string") {
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
                    onKeyDown={async (e): Promise<void> => {
                        if (e.key === "Enter") {
                            e.preventDefault();
                            await router.push({
                                pathname: "/search",
                                query: { query, page: 1 },
                            });
                        }
                    }}
                />
                <InputRightElement>
                    <SearchIcon />
                </InputRightElement>
            </InputGroup>
        </Container>
    );
}
