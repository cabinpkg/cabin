import "../styles/globals.css";
import type { AppProps } from "next/app";
import { ChakraProvider } from "@chakra-ui/react";
import { UserProvider } from "@supabase/supabase-auth-helpers/react";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";
import NextNProgress from "nextjs-progressbar";

import Layout from "~/components/Layout";
import theme from "~/utils/theme";

function MyApp({ Component, pageProps }: AppProps): JSX.Element {
    return (
        <ChakraProvider theme={theme}>
            <NextNProgress />
            <UserProvider supabaseClient={supabaseClient}>
                <Layout>
                    <Component {...pageProps} />
                </Layout>
            </UserProvider>
        </ChakraProvider>
    );
}

export default MyApp;
