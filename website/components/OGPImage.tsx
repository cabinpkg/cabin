// ref: https://zenn.dev/66ed3gs/articles/55a12d957d6b7e

import Head from "next/head";
import { ChakraProvider, Flex, Heading, VStack, Image, Text, Center } from "@chakra-ui/react";

import { BASE_URL } from "~/utils/constants";

interface OGPImageProps {
    name: string;
    version: "latest" | string;
    description: string;
}

const OGPImage = ({ name, version, description }: OGPImageProps): JSX.Element => (
    <>
        <Head>
            <link />
        </Head>
        <ChakraProvider>
            <Flex padding={8} width="full" height="full" align="center" justify="center"
                bgGradient="linear(to-tr, #3023AE 0%, #53A0FD 80%, #51DEEC 100%)"
            >
                <Center width="99%" height="99%" borderRadius="2xl" bgColor="white" paddingX={20} paddingY={10}>
                    <VStack>
                        <Heading fontSize="4em" textAlign="center" style={{ whiteSpace: "pre-wrap" }}>
                            {name}
                        </Heading>
                        <Text fontSize="2em" textAlign="center">({version !== "latest" && "v"}{version})</Text>
                        <Text fontSize="2em" textAlign="center">{description}</Text>
                    </VStack>
                </Center>
                <Image
                    width="200px"
                    src={`${BASE_URL}/logo.svg`}
                    alt="Logo"
                    position="absolute"
                    bottom="70px"
                    right="90px"
                />
            </Flex>
        </ChakraProvider>
    </>
);

export default OGPImage;
