import { unified } from "@astrojs/markdown-remark";
import sitemap from "@astrojs/sitemap";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";
import rehypeAutolinkHeadings from "rehype-autolink-headings";
import rehypeKatex from "rehype-katex";
import rehypeSlug from "rehype-slug";
import remarkMath from "remark-math";
import { SITE_URL } from "./src/lib/constants";
import { remarkDocsLinks } from "./src/lib/remark-docs-links";

// The dev-server proxy target for the registry routes (see `vite.server`
// below). `changeOrigin` makes the upstream see the production Host so the
// Worker's hostname-role split picks the website plane.
const proxyToProduction = {
    target: SITE_URL,
    changeOrigin: true,
};

export default defineConfig({
    site: SITE_URL,
    output: "static",
    integrations: [sitemap()],
    // Prefetch in-site links (on hover/tap) so docs page-to-page navigation
    // feels instant. Uses <link rel="prefetch"> with a same-origin fetch
    // fallback, and the prefetch runtime is bundled as an external script —
    // both satisfy the strict CSP (script-src 'self', connect-src 'self').
    prefetch: { prefetchAll: true },
    markdown: {
        // Cool terminal palette for code blocks, matching the site's
        // steel/pine color system (the block background itself is pinned to
        // the surface scale in global.css).
        shikiConfig: { theme: "nord" },
        // Build the default Astro markdown pipeline (GFM, Shiki, heading IDs)
        // plus: remark steps mapping the docs' relative `*.md` cross-links to
        // `/docs/<slug>/` and parsing `$`/`$$` math, and rehype steps that
        // render the math with KaTeX (server-side; the stylesheet is imported
        // by `DocsLayout.astro`), give each heading an id, and wrap it in a
        // self-link (`src/scripts/docs.ts` turns clicking that link into
        // "copy deep link"). `processor` is the non-deprecated replacement
        // for the top-level `markdown.remarkPlugins` option; user rehype
        // plugins run before Astro's own heading-id step, so `rehype-slug`
        // supplies the ids that `rehype-autolink-headings` needs.
        processor: unified({
            remarkPlugins: [remarkDocsLinks, remarkMath],
            rehypePlugins: [
                rehypeKatex,
                rehypeSlug,
                [
                    rehypeAutolinkHeadings,
                    {
                        // Append a small "#" affordance to each heading (styled
                        // in global.css). Only this anchor is the copy target —
                        // the heading text itself stays non-clickable.
                        behavior: "append",
                        content: [],
                        properties: {
                            className: ["heading-anchor"],
                            ariaLabel: "Copy link to this section",
                        },
                    },
                ],
            ],
        }),
    },
    vite: {
        plugins: [tailwindcss()],
        build: {
            // Never inline scripts, so bundled page scripts (e.g. the docs
            // enhancer) are emitted as external files and the strict CSP
            // (`script-src 'self'`, no inline scripts) holds.
            assetsInlineLimit: 0,
        },
        // Dev-server-only (`astro dev`; Vite server options never reach the
        // static build): forward the registry routes mounted on the website
        // origin to production, so the account pages' relative fetches and
        // the host-only session cookie work from localhost. `/login` is
        // matched exactly - `/login/denied` is a static page of this site
        // and must not be proxied.
        server: {
            proxy: {
                "^/api/": proxyToProduction,
                "^/login(?:\\?|$)": proxyToProduction,
                "^/callback(?:\\?|$)": proxyToProduction,
            },
        },
    },
});
