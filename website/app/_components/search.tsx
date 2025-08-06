"use client";

import { faMagnifyingGlass } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Input, Spinner } from "@heroui/react";
import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";

export function SearchButton() {
    const [value, setValue] = useState("");
    const [isPending, startTransition] = useTransition();
    const router = useRouter();

    const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
        if (event.key === "Enter" && value.trim()) {
            startTransition(() => {
                router.push(`/search?q=${encodeURIComponent(value.trim())}`);
            });
        }
    };

    const handleSubmit = (event: React.FormEvent) => {
        event.preventDefault();
        if (value.trim()) {
            startTransition(() => {
                router.push(`/search?q=${encodeURIComponent(value.trim())}`);
            });
        }
    };

    return (
        <form onSubmit={handleSubmit} className="w-full">
            <Input
                type="search"
                placeholder="Search packages..."
                aria-label="Search packages"
                labelPlacement="outside"
                size="sm"
                radius="lg"
                className="w-full"
                classNames={{
                    input: "text-sm",
                    inputWrapper:
                        "h-10 shadow-sm hover:shadow-md transition-shadow",
                }}
                startContent={
                    isPending ? (
                        <Spinner size="sm" className="text-default-600" />
                    ) : (
                        <FontAwesomeIcon
                            className="text-default-600 dark:text-default-500"
                            icon={faMagnifyingGlass}
                            width={14}
                        />
                    )
                }
                value={value}
                onValueChange={setValue}
                onKeyDown={handleKeyDown}
                isDisabled={isPending}
            />
        </form>
    );
}
