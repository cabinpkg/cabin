import type { NextApiRequest as Req, NextApiResponse as Res } from "next";
import { renderToStaticMarkup } from "react-dom/server";
import { chromium } from "playwright";
import truncate from "truncate";

import OGPImage from "~/components/OGPImage";

export default async function ogp(req: Req, res: Res): Promise<void> {
    const name = (req.query.name || "").toString();
    const version = (req.query.ver || "").toString();
    const description = (req.query.desc || "").toString();
    if (name === "" || version === "" || description === "") {
        return res.status(400).send("Bad request");
    }

    // Set image size of an OGP image
    const viewport = { width: 1200, height: 630 };

    // Launch a headless browser
    const browser = await chromium.launch({ headless: true });
    const page = await browser.newPage({ viewport });

    // Make HTML from `OGPImage`
    const markup = renderToStaticMarkup(<OGPImage name={name} version={version} description={truncate(description, 190)} />);

    // Use `networkidle` to wait for loading images
    await page.setContent(markup, { waitUntil: "networkidle" });

    // Load fonts
    await page.addStyleTag({
        url: "https://fonts.googleapis.com/css2?family=M+PLUS+1p:wght@400;500;700&family=Noto+Sans+JP:wght@100;300;400;500;700;900&display=swap",
    });

    // Take a screenshot on a page
    const buf = await page.screenshot({ type: "png" });
    await browser.close();

    // Return a response as the generated image
    res.setHeader("Cache-Control", "s-maxage=2592000, stale-while-revalidate");
    res.setHeader("Content-Type", "image/png");
    res.end(buf);
}
