import type { DocumentContext, DocumentInitialProps } from "next/document";
import Document, { Html, Head, Main, NextScript } from "next/document";
import { ColorModeScript } from "@chakra-ui/react";
import Script from "next/script";

import theme from "~/utils/theme";

class MyDocument extends Document {
    static async getInitialProps(ctx: DocumentContext): Promise<DocumentInitialProps> {
        const initialProps = await Document.getInitialProps(ctx);
        return { ...initialProps };
    }

    render(): JSX.Element {
        return (
            <Html>
                <Head>
                    <title>Poac - Package Manager for C++</title>
                    <Script
                        src="https://www.googletagmanager.com/gtag/js?id=G-H8CM4YENLC"
                        strategy="afterInteractive"
                    />
                    <Script id="google-analytics" strategy="afterInteractive">
                        {`
                          window.dataLayer = window.dataLayer || [];
                          function gtag(){window.dataLayer.push(arguments);}
                          gtag('js', new Date());
                
                          gtag('config', 'G-H8CM4YENLC');
                        `}
                    </Script>
                </Head>
                <body>
                    <ColorModeScript initialColorMode={theme.config.initialColorMode} />
                    <Main />
                    <NextScript />
                </body>
            </Html>
        );
    }
}

export default MyDocument;
