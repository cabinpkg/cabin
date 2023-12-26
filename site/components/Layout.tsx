import { VStack } from "@chakra-ui/react";
import type { ReactElement } from "react";

import Footer from "~/components/Footer";
import Header from "~/components/Header";

export default function Layout({
    children,
}: { children: ReactElement }): JSX.Element {
    return (
        <VStack spacing={10} align="right" margin={5}>
            <Header />
            {children}
            <Footer />
        </VStack>
    );
}
