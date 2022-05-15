import { Divider, VStack, HStack, LinkBox, LinkOverlay } from "@chakra-ui/react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faGithub } from "@fortawesome/free-brands-svg-icons";
import { faBookOpen, faHandHoldingHeart } from "@fortawesome/free-solid-svg-icons";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";

interface LinkWithIconProps {
    icon: IconProp;
    href: string;
    name: string;
}

function LinkWithIcon(props: LinkWithIconProps): JSX.Element {
    return (
        <LinkBox>
            <HStack spacing={1}>
                <FontAwesomeIcon icon={props.icon} />
                <LinkOverlay href={props.href} isExternal>{props.name}</LinkOverlay>
            </HStack>
        </LinkBox>
    );
}

export default function Footer(): JSX.Element {
    return (
        <VStack spacing={5}>
            <Divider />
            <HStack spacing={10}>
                <LinkWithIcon icon={faBookOpen} href="https://doc.poac.pm/" name="Docs" />
                <LinkWithIcon icon={faGithub} href="https://github.com/poacpm" name="GitHub" />
                <LinkWithIcon icon={faHandHoldingHeart} href="https://github.com/sponsors/ken-matsui" name="Sponsor" />
            </HStack>
        </VStack>
    );
}
