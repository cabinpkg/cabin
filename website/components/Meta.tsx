import Head from "next/head";

interface MetaProps {
    title?: string;
}

export default function Meta(props: MetaProps): JSX.Element {
    return (
        <Head>
            <title>{props.title ? `${props.title} - Poac: Package Manager for C++`: "Poac: Package Manager for C++"}</title>
            <meta name="author" content="matken" />

            <meta name="twitter:card" content="summary" />
            <meta name="twitter:site" content="@_matken" />

            <meta property="og:title" content="Poac Package Manager for C++" />
            <meta property="og:site_name" content="poac" />
            <meta property="og:type" content="website" />
            <meta property="og:url" content="https://poac.pm" />
            <meta property="og:image" content="https://poac.pm/icon-512x512.png" />
            {/* TODO: generate description dynamically */}
            <meta property="og:description" content="Package Manager for C++." />
            <meta name="theme-color" content="#000000" />
            {/*
                manifest.json provides metadata used when your web app is added to the
                homescreen on Android. See https://developers.google.com/web/fundamentals/engage-and-retain/web-app-manifest/
            */}
            <link rel="manifest" href="/manifest.json" />
            <link rel="apple-touch-icon" sizes="152x152" href="/apple-touch-icon.png" />
        </Head>
    );
}
