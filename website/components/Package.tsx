import {
    Heading,
    HStack,
    LinkBox,
    Spacer,
    Tag,
    Text,
    VStack,
} from "@chakra-ui/react";

import { LinkOverlay } from "~/components/Link";
import type { PackageOverview } from "~/utils/types";

interface PackageProps {
    package: PackageOverview;
}

export default function Package(props: PackageProps): JSX.Element {
    return (
        <LinkBox
            borderWidth="1px"
            borderRadius="md"
            boxShadow="md"
            padding={5}
            width="30vw"
            minWidth="100%"
        >
            <HStack>
                <VStack align="left">
                    <HStack spacing={3}>
                        <Heading size="sm" my="2">
                            <LinkOverlay
                                href={`/packages/${props.package.name}`}
                            >
                                {props.package.name}
                            </LinkOverlay>
                        </Heading>
                        <Text>v{props.package.version}</Text>
                    </HStack>
                    <Text>{props.package.description}</Text>
                </VStack>
                <Spacer />
                <Tag>C++{props.package.edition.toString().slice(-2)}</Tag>
            </HStack>
        </LinkBox>
    );
}
