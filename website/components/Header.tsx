import { MoonIcon, SunIcon } from "@chakra-ui/icons";
import {
    Center,
    Flex,
    IconButton,
    Image,
    LinkBox,
    Spacer,
    useColorMode,
    useColorModeValue,
} from "@chakra-ui/react";
import { useRouter } from "next/router";

import { Link, LinkOverlay } from "~/components/Link";
import Search from "~/components/Search";

export default function Header(): JSX.Element {
    const router = useRouter();
    const { toggleColorMode } = useColorMode();
    const colorModeIcon = useColorModeValue(<MoonIcon />, <SunIcon />);
    const logoName = useColorModeValue("/logo-black.svg", "/logo-white.svg");

    return (
        <Flex>
            <Spacer />
            <LinkBox>
                <LinkOverlay href="/">
                    <Image width="80px" src={logoName} alt="Logo" />
                </LinkOverlay>
            </LinkBox>
            {router.pathname === "/" ? (
                <>
                    <Spacer />
                    <Center>
                        <Link href="/search" marginRight={5}>
                            Browse all packages
                        </Link>
                    </Center>
                </>
            ) : (
                <Search />
            )}
            <IconButton
                onClick={toggleColorMode}
                aria-label="Toggle theme"
                icon={colorModeIcon}
                marginRight={5}
            />
            <Spacer />
        </Flex>
    );
}
