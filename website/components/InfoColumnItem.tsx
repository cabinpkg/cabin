import { Code, HStack, Text, VStack } from "@chakra-ui/react";
import { ReactElement } from "react";

interface Props {
    title: string;
    count?: number;
    children: ReactElement | ReactElement[];
}

export default function InfoColumnItem(props: Props): JSX.Element {
    return (
        <VStack width="100%" align="left">
            <HStack>
                <Text as="b" fontSize="lg" marginBottom={2}>
                    {props.title}
                </Text>
                {props.count && <Code>{props.count}</Code>}
            </HStack>
            {props.children}
        </VStack>
    );
}
