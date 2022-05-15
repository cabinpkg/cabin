import "../styles/globals.css";
import type { AppProps } from "next/app";
import { ChakraProvider } from "@chakra-ui/react";
import { UserProvider } from "@supabase/supabase-auth-helpers/react";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";

import Layout from "../components/Layout";

function MyApp({ Component, pageProps }: AppProps): JSX.Element {
    return (
        <ChakraProvider>
            <UserProvider supabaseClient={supabaseClient}>
                <Layout>
                    <Component {...pageProps} />
                </Layout>
            </UserProvider>
        </ChakraProvider>
    );
}

export default MyApp;
