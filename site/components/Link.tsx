// ref: https://github.com/chakra-ui/chakra-ui/discussions/3152#discussioncomment-309850

import type { LinkProps, LinkOverlayProps } from "@chakra-ui/react";
import { forwardRef, Link as ChakraLink, LinkOverlay as ChakraLinkOverlay } from "@chakra-ui/react";
import { ExternalLinkIcon } from "@chakra-ui/icons";
import NextLink from "next/link";

type LinkCompoundType = LinkProps & {
    href: string;
}

export const Link = forwardRef<LinkCompoundType, "a">(({ children, href, isExternal, ...restProps }, ref) => (
    <NextLink href={href} passHref>
        <ChakraLink ref={ref} isExternal={isExternal} {...restProps}>
            {children} {isExternal && <ExternalLinkIcon mx="2px" />}
        </ChakraLink>
    </NextLink>
));

type LinkOverlayCompoundType = LinkOverlayProps & {
    href: string;
}

export const LinkOverlay = forwardRef<LinkOverlayCompoundType, "a">(({ href, ...restProps }, ref) => (
    <NextLink href={href} passHref>
        <ChakraLinkOverlay ref={ref} {...restProps} />
    </NextLink>
));
