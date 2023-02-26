import type { DocumentContext, DocumentInitialProps } from "next/document";
import Document, { Html, Head, Main, NextScript } from "next/document";
import { ColorModeScript } from "@chakra-ui/react";

import theme from "~/utils/theme";

class MyDocument extends Document {
    static async getInitialProps(
        ctx: DocumentContext,
    ): Promise<DocumentInitialProps> {
        const initialProps = await Document.getInitialProps(ctx);
        return { ...initialProps };
    }

    render(): JSX.Element {
        return (
            <Html>
                <Head />
                <body>
                    <ColorModeScript
                        initialColorMode={theme.config.initialColorMode}
                    />
                    <Main />
                    <NextScript />
                </body>
            </Html>
        );
    }
}

export default MyDocument;
