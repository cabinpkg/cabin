import { Center, Heading, VStack } from "@chakra-ui/react";

import LoginButton from "~/components/LoginButton";

export default function NeedAuth(): JSX.Element {

    return (
        <Center>
            <VStack spacing={5}>
                <Heading>This page needs authentication</Heading>
                <LoginButton />
            </VStack>
        </Center>
    );
}
