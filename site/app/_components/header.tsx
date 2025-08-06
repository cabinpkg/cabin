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
            {/* Mobile layout */}
            <div className="flex sm:hidden w-full items-center gap-4 px-4">
                <NextLink href="/" className="flex-shrink-0">
                    <Logo />
                </NextLink>
                <div className="flex-1">
                    <SearchButton />
                </div>
            </div>

            {/* Desktop layout */}
            <div className="hidden sm:flex w-full items-center">
                <NavbarBrand className="flex-shrink-0">
                    <NextLink href="/" className="flex items-center gap-2">
                        <Logo />
                    </NextLink>
                </NavbarBrand>

                <NavbarContent className="flex-1 justify-center px-4">
                    <NavbarItem className="w-full max-w-2xl">
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
                            className="text-default-600 hover:text-primary transition-colors"
                            startContent={
                                <FontAwesomeIcon
                                    icon={faBookOpen}
                                    className="text-sm"
                                />
                            }
                        >
                            Docs
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
                            <FontAwesomeIcon
                                icon={faGithub}
                                className="text-lg"
                            />
                        </Button>
                    </NavbarItem>

                    {/* Sponsor link */}
                    <NavbarItem>
                        <Button
                            as={Link}
                            href="https://github.com/sponsors/ken-matsui"
                            isExternal
                            color="danger"
                            variant="flat"
                            size="sm"
                            className="font-medium shadow-sm hover:shadow-md transition-shadow"
                            startContent={
                                <FontAwesomeIcon
                                    icon={faHeart}
                                    className="text-sm animate-pulse"
                                />
                            }
                        >
                            Sponsor
                        </Button>
                    </NavbarItem>
                </NavbarContent>
            </div>
        </Navbar>
    );
}
