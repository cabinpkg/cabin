import { DownloadIcon, InfoIcon } from "@chakra-ui/icons";
import { Button, HStack, Heading, VStack } from "@chakra-ui/react";
import type { NextPage } from "next";

import Meta from "~/components/Meta";
import Search from "~/components/Search";

const Home: NextPage = () => {
    return (
        <>
            <Meta />
            <VStack spacing={5}>
                <Heading as="h1">Package Manager for C++ Developers</Heading>
                <Heading as="h2" size="md">
                    Poac is a C++ package manager which is for open source
                    software.
                    <br />
                    Easy to introduce to your projects; you can use packages
                    intuitively.
                </Heading>
                <Search />
                <HStack>
                    <Button
                        as="a"
                        href="https://github.com/poac-dev/poac#installation"
                        leftIcon={<DownloadIcon />}
                    >
                        Install Poac
                    </Button>
                    <Button
                        as="a"
                        href="https://doc.poac.dev/guide"
                        leftIcon={<InfoIcon />}
                    >
                        Getting Started
                    </Button>
                </HStack>
            </VStack>
        </>
    );
};

export default Home;
