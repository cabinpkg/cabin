import type { NextPage } from "next";
import { VStack, Heading, Image } from "@chakra-ui/react";

import Search from "../components/Search";

const Home: NextPage = () => {
    return (
        <VStack spacing={5}>
            <Image src="/terminal.svg" maxWidth={700} alt="demo" />
            <Heading as="h1">
                Package Manager for C++ Developers
            </Heading>
            <Heading as="h2" size="md">
                Poac is a C++ package manager which is for open source software.<br/>
                Easy to introduce to your projects; you can use packages intuitively.
            </Heading>
            <Search />
        </VStack>
    );
};

export default Home;
