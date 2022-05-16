import { Heading, HStack, Center } from "@chakra-ui/react";

import Meta from "~/components/Meta";

export default function Custom404(): JSX.Element {
    return (
        <>
            <Meta title="404: Not Found" />
            <Center height="50px">
                <HStack>
                    <Heading>404</Heading>
                    <Heading size="md">This page could not be found.</Heading>
                </HStack>
            </Center>
        </>
    );
}
