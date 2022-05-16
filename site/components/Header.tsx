import { Flex, IconButton, useColorMode, useColorModeValue, Spacer, Image, LinkBox, LinkOverlay, Link, Center } from "@chakra-ui/react";
import { MoonIcon, SunIcon } from "@chakra-ui/icons";
import { useRouter } from "next/router";

import UserMenu from "~/components/UserMenu";
import Search from "~/components/Search";

export default function Header(): JSX.Element {
    const router = useRouter();
    const { toggleColorMode } = useColorMode();
    const colorModeIcon = useColorModeValue(<MoonIcon />, <SunIcon />);

    return (
        <Flex>
            <Spacer />
            <LinkBox>
                <LinkOverlay href="/">
                    <Image width="80px" src="/logo.svg" alt="Icon" />
                </LinkOverlay>
            </LinkBox>
            {router.pathname === "/" ?
                <>
                    <Spacer />
                    <Center>
                        <Link href="/search" marginRight={5}>Browse all packages</Link>
                    </Center>
                </> :
                <Search />
            }
            <IconButton
                onClick={toggleColorMode}
                aria-label="Toggle theme"
                icon={colorModeIcon}
                marginRight={5}
            />
            <UserMenu />
            <Spacer />
        </Flex>
    );
}
