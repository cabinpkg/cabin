import { faGithub } from "@fortawesome/free-brands-svg-icons";
import { faBookOpen, faHeart } from "@fortawesome/free-solid-svg-icons";
import { Navbar, NavbarBrand, NavbarContent, NavbarItem } from "@heroui/react";
import NextLink from "next/link";
import { ExternalLinkButton } from "./external-link-button";

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
                        <ExternalLinkButton
                            href="https://docs.cabinpkg.com"
                            icon={faBookOpen}
                            variant="light"
                            className="text-default-600 hover:text-primary transition-colors"
                        >
                            Docs
                        </ExternalLinkButton>
                    </NavbarItem>

                    {/* GitHub link */}
                    <NavbarItem>
                        <ExternalLinkButton
                            href="https://github.com/cabinpkg"
                            icon={faGithub}
                            variant="light"
                            className="text-default-600 hover:text-primary transition-colors"
                            isIconOnly
                            iconClassName="text-lg"
                        >
                            GitHub Repository
                        </ExternalLinkButton>
                    </NavbarItem>

                    {/* Sponsor link */}
                    <NavbarItem>
                        <ExternalLinkButton
                            href="https://github.com/sponsors/ken-matsui"
                            icon={faHeart}
                            color="danger"
                            className="font-medium shadow-sm hover:shadow-md transition-shadow"
                            iconClassName="animate-pulse"
                        >
                            Sponsor
                        </ExternalLinkButton>
                    </NavbarItem>
                </NavbarContent>
            </div>
        </Navbar>
    );
}
