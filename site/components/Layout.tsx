import { VStack } from "@chakra-ui/react";
import type { ReactElement } from "react";

import Header from "./Header";
import Footer from "./Footer";

export default function Layout({ children }: {children: ReactElement}): JSX.Element {
    return (
        <VStack spacing={10} align="right" marginY={5}>
            <Header />
            {children}
            <Footer />
        </VStack>
    );
}
