import Head from "next/head";
import { useRouter } from "next/router";
import truncate from "truncate";

import type { Package } from "~/utils/types";

function constructOgImageUrl(
    name: string,
    version: string,
    description: string,
): string {
    const ogImageUrlLeft = "https://res.cloudinary.com/poac/image/upload";
    const ogImageUrlRight = "opg_bg.png";

    const cldFit = "w_1000,c_fit";
    const cldFont = "l_text:Roboto";
    const cldHeading = `${cldFont}_70_bold_center`;
    const cldText = `${cldFont}_35_center`;
    const cldRelativeBottom = (y: number): string =>
        `fl_layer_apply,y_${y}_add_$lineHeight`;

    const ogImageName = `${cldFit},${cldHeading}:${name.replace(
        "/",
        "%252F",
    )},$lineHeight_h`;
    const ogImageVersion = `${cldFit},${cldText}:(${
        version !== "latest" ? "v" : ""
    }${version})/${cldRelativeBottom(10)}`;
    const ogImageDesc = `${cldFit},${cldText}:${truncate(
        description,
        180,
    )}/${cldRelativeBottom(70)}`;

    return `${ogImageUrlLeft}/${ogImageName}/${ogImageVersion}/${ogImageDesc}/${ogImageUrlRight}`;
}

interface MetaProps {
    title?: string;
    package?: Pick<Package, "name" | "version">;
    description?: string;
}

export default function Meta(props: MetaProps): JSX.Element {
    const router = useRouter();

    const title = props.title
        ? `${props.title} - Poac`
        : "Poac: Package Manager for C++";
    const description = props.description ?? "Package Manager for C++.";
    const ogImagePath =
        props.package && props.description
            ? constructOgImageUrl(
                  props.package.name,
                  props.package.version,
                  props.description,
              )
            : "https://poac.dev/icon-512x512.png";

    return (
        <Head>
            <title>{title}</title>
            <meta name="author" content="Ken Matsui" />

            <meta name="twitter:card" content="summary_large_image" />

            <meta
                property="og:url"
                content={`https://poac.dev${router.asPath}`}
            />
            <meta property="og:title" content={title} />
            <meta property="description" content={description} />
            <meta property="og:description" content={description} />
            <meta property="og:image" content={ogImagePath} />
            <meta property="og:site_name" content="poac.dev" />
            <meta property="og:type" content="website" />

            <meta name="theme-color" content="#000000" />
            {/*
                manifest.json provides metadata used when your web app is added to the
                homescreen on Android. See https://developers.google.com/web/fundamentals/engage-and-retain/web-app-manifest/
            */}
            <link rel="manifest" href="/manifest.json" />
            <link
                rel="apple-touch-icon"
                sizes="152x152"
                href="/apple-touch-icon.png"
            />
        </Head>
    );
}
