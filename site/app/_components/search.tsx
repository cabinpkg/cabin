"use client";

import { faMagnifyingGlass } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Input } from "@nextui-org/react";
import { useRouter } from "next/navigation";
import { useState } from "react";

export function SearchButton() {
    const [value, setValue] = useState("");
    const router = useRouter();

    const handleSearch = (e: React.FormEvent) => {
        e.preventDefault(); // Prevent the form from refreshing the page
        router.push(`/search?q=${value}`);
    };

    return (
        <form onSubmit={handleSearch}>
            <Input
                type="search"
                placeholder="Search packages"
                aria-label="Search packages"
                labelPlacement="outside"
                startContent={
                    <FontAwesomeIcon
                        className="text-default-600 dark:text-default-500"
                        icon={faMagnifyingGlass}
                        width={13}
                    />
                }
                value={value}
                onValueChange={setValue}
            >
                Search packages
            </Input>
        </form>
    );
}
