import { Heading, HStack, LinkBox, LinkOverlay, Text } from "@chakra-ui/react";

import type { Package } from "../types";

interface PackageProps {
    package: Package;
    group?: string;
}

export default function Package(props: PackageProps): JSX.Element {
    return (
        <LinkBox borderWidth="1px" borderRadius="md" boxShadow="md" padding={5} minWidth="100%">
            <HStack spacing={3}>
                <Heading size="sm" my="2">
                    <LinkOverlay href={"/packages/" + props.package.name}>
                        {props.group ? props.package.name.replace(props.group + "/", "") : props.package.name}
                    </LinkOverlay>
                </Heading>
                <Text>v{props.package.version}</Text>
            </HStack>
            <Text>{props.package.description}</Text>
        </LinkBox>
    );
}
