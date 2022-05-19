import Head from "next/head";
import { useRouter } from "next/router";

import { BASE_URL } from "~/utils/constants";
import type { Package } from "~/utils/types";

interface MetaProps {
    title?: string;
    package?: Pick<Package, "name" | "version">;
    description?: string;
}

export default function Meta(props: MetaProps): JSX.Element {
    const router = useRouter();

    const title = props.title ? `${props.title} - Poac`: "Poac: Package Manager for C++";
    const description = props.description ?? "Package Manager for C++.";
    const ogImagePath = props.package && props.description ?
        `${BASE_URL}/api/ogp?name=${props.package.name}&ver=${props.package.version}&desc=${description}` :
        "https://poac.pm/icon-512x512.png";

    return (
        <Head>
            <title>{title}</title>
            <meta name="author" content="Ken Matsui" />

            <meta name="twitter:card" content="summary_large_image" />

            <meta property="og:url" content={`https://poac.pm${router.asPath}`} />
            <meta property="og:title" content={title} />
            <meta property="description" content={description} />
            <meta property="og:description" content={description} />
            <meta property="og:image" content={ogImagePath} />
            <meta property="og:site_name" content="Poac.pm" />
            <meta property="og:type" content="website" />

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
