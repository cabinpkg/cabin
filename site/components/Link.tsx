// ref: https://github.com/chakra-ui/chakra-ui/discussions/3152#discussioncomment-309850

import { ExternalLinkIcon } from "@chakra-ui/icons";
import type { LinkOverlayProps, LinkProps } from "@chakra-ui/react";
import {
    Link as ChakraLink,
    LinkOverlay as ChakraLinkOverlay,
    forwardRef,
} from "@chakra-ui/react";
import NextLink from "next/link";

type LinkCompoundType = LinkProps & {
    href: string;
};

export const Link = forwardRef<LinkCompoundType, "a">(
    ({ children, href, isExternal, ...restProps }, ref) => (
        <ChakraLink
            as={NextLink}
            href={href}
            ref={ref}
            isExternal={isExternal}
            {...restProps}
        >
            {children} {isExternal && <ExternalLinkIcon mx="2px" />}
        </ChakraLink>
    ),
);

type LinkOverlayCompoundType = LinkOverlayProps & {
    href: string;
};

export const LinkOverlay = forwardRef<LinkOverlayCompoundType, "a">(
    ({ href, ...restProps }, ref) => (
        <ChakraLinkOverlay as={NextLink} href={href} ref={ref} {...restProps} />
    ),
);
