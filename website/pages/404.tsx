import { Heading, HStack, Center } from "@chakra-ui/react";

export default function Custom404(): JSX.Element {
    return (
        <Center height="50px">
            <HStack>
                <Heading>404</Heading>
                <Heading size="md">This page could not be found.</Heading>
            </HStack>
        </Center>
    );
}
