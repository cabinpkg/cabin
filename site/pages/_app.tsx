import { ChakraProvider } from "@chakra-ui/react";
import { Analytics } from "@vercel/analytics/react";
import type { AppProps } from "next/app";
import NextNProgress from "nextjs-progressbar";
import "../styles/globals.css";

import Layout from "~/components/Layout";
import theme from "~/utils/theme";

function MyApp({ Component, pageProps }: AppProps): JSX.Element {
    return (
        <>
            <ChakraProvider theme={theme}>
                <NextNProgress options={{ showSpinner: false }} />
                <Layout>
                    <Component {...pageProps} />
                </Layout>
            </ChakraProvider>
            <Analytics />
        </>
    );
}

export default MyApp;
