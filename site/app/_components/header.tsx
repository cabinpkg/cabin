import { faGithub } from "@fortawesome/free-brands-svg-icons";
import { faBookOpen, faHeart } from "@fortawesome/free-solid-svg-icons";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
    Button,
    Link,
    Navbar,
    NavbarBrand,
    NavbarContent,
    NavbarItem,
} from "@heroui/react";
import NextLink from "next/link";

import { Logo } from "./logo";
import { SearchButton } from "./search";

export function Header() {
    return (
        <Navbar
            className="backdrop-blur-md bg-background/80 border-b border-divider"
            height="70px"
            maxWidth="full"
            position="sticky"
        >
            <NavbarBrand className="flex-shrink-0">
                <NextLink href="/" className="flex items-center gap-2">
                    <Logo />
                </NextLink>
            </NavbarBrand>

            {/* Center search - always centered */}
            <NavbarContent className="flex-1" justify="center">
                <NavbarItem className="w-full max-w-2xl md:max-w-2xl max-w-xs">
                    <SearchButton />
                </NavbarItem>
            </NavbarContent>

            {/* Right side navigation */}
            <NavbarContent className="gap-2 flex-shrink-0" justify="end">
                {/* Docs link */}
                <NavbarItem>
                    <Button
                        as={Link}
                        href="https://docs.cabinpkg.com"
                        isExternal
                        variant="light"
                        size="sm"
                        className="text-default-600 hover:text-primary transition-colors min-w-unit-8 sm:min-w-unit-16"
                        startContent={
                            <FontAwesomeIcon
                                icon={faBookOpen}
                                className="text-sm"
                            />
                        }
                    >
                        <span className="hidden sm:inline">Docs</span>
                    </Button>
                </NavbarItem>

                {/* GitHub link */}
                <NavbarItem>
                    <Button
                        as={Link}
                        href="https://github.com/cabinpkg"
                        isExternal
                        variant="light"
                        size="sm"
                        className="text-default-600 hover:text-primary transition-colors"
                        isIconOnly
                        aria-label="GitHub Repository"
                    >
                        <FontAwesomeIcon icon={faGithub} className="text-lg" />
                    </Button>
                </NavbarItem>

                <NavbarItem>
                    <Button
                        as={Link}
                        href="https://github.com/sponsors/ken-matsui"
                        isExternal
                        color="danger"
                        variant="flat"
                        size="sm"
                        className="font-medium shadow-sm hover:shadow-md transition-shadow min-w-unit-8 sm:min-w-unit-16"
                        startContent={
                            <FontAwesomeIcon
                                icon={faHeart}
                                className="text-sm animate-pulse"
                            />
                        }
                    >
                        <span className="hidden sm:inline">Sponsor</span>
                    </Button>
                </NavbarItem>
            </NavbarContent>
        </Navbar>
    );
}
