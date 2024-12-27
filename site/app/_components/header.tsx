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
} from "@nextui-org/react";
import NextLink from "next/link";

import { Logo } from "./logo";
import { SearchButton } from "./search";

export function Header() {
    return (
        <Navbar>
            <NavbarBrand>
                <NextLink href="/">
                    <Logo />
                </NextLink>
            </NavbarBrand>
            <NavbarContent justify="center">
                <NavbarItem>
                    <SearchButton />
                </NavbarItem>
            </NavbarContent>
            <NavbarContent className="hidden sm:flex gap-4" justify="end">
                <NavbarItem>
                    <Link
                        isExternal
                        aria-label="Docs"
                        className="p-1 text-default-600 dark:text-default-500 text-sm"
                        href="https://docs.cabinpkg.com"
                    >
                        <FontAwesomeIcon
                            className="text-default-600 dark:text-default-500"
                            icon={faBookOpen}
                            width={20}
                        />
                        &nbsp;Docs
                    </Link>
                </NavbarItem>
                <NavbarItem>
                    <Link
                        isExternal
                        aria-label="Github"
                        className="p-1"
                        href="https://github.com/cabinpkg"
                    >
                        <FontAwesomeIcon
                            className="text-default-600 dark:text-default-500"
                            icon={faGithub}
                            width={20}
                        />
                    </Link>
                </NavbarItem>
                <NavbarItem>
                    <Button
                        isExternal
                        as={Link}
                        className="group text-sm font-normal text-default-600 bg-default-400/20 dark:bg-default-500/20"
                        href="https://github.com/sponsors/ken-matsui"
                        startContent={
                            <FontAwesomeIcon
                                className="text-danger"
                                icon={faHeart}
                                width={15}
                            />
                        }
                        variant="flat"
                    >
                        Sponsor
                    </Button>
                </NavbarItem>
            </NavbarContent>
        </Navbar>
    );
}
