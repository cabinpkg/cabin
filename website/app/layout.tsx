import { Analytics } from "@vercel/analytics/react";
import { SpeedInsights } from "@vercel/speed-insights/next";

import { Footer } from "./_components/footer";
import { Header } from "./_components/header";
import { Providers } from "./providers";

import "./globals.css";

export default function Layout({
    children,
}: {
    children: React.ReactNode;
}): JSX.Element {
    return (
        <html lang="en" className="dark">
            <body
                className={"min-h-screen bg-background font-sans antialiased"}
            >
                <Providers>
                    <Header />
                    {children}
                    <Footer />
                </Providers>
                <Analytics />
                <SpeedInsights />
            </body>
        </html>
    );
}
