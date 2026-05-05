import MarkdownIt from "markdown-it";
import type Token from "markdown-it/lib/token.mjs";

let markdown: MarkdownIt | undefined;

export function renderReadmeMarkdown(readme: string): string {
    markdown ??= createMarkdownRenderer();
    return markdown.render(readme);
}

function createMarkdownRenderer(): MarkdownIt {
    const renderer = new MarkdownIt({
        html: false,
        linkify: true,
        typographer: true,
    });
    const defaultLinkOpen = renderer.renderer.rules.link_open;
    const defaultImage = renderer.renderer.rules.image;

    renderer.renderer.rules.link_open = (...args) => {
        const [tokens, index, options, env, self] = args;
        tokens[index].attrSet("target", "_blank");
        tokens[index].attrSet("rel", "noopener noreferrer");

        return defaultLinkOpen
            ? defaultLinkOpen(tokens, index, options, env, self)
            : self.renderToken(tokens, index, options);
    };

    renderer.renderer.rules.image = (...args) => {
        const [tokens, index, options, env, self] = args;
        const src = tokens[index].attrGet("src");

        // Relative README images are intentionally unsupported. Resolving them
        // correctly requires upstream repository context we do not reliably have.
        if (!isAbsoluteHttpUrl(src)) {
            removeTokenAttribute(tokens[index], "src");
        }

        return defaultImage
            ? defaultImage(tokens, index, options, env, self)
            : self.renderToken(tokens, index, options);
    };

    return renderer;
}

function removeTokenAttribute(token: Token, name: string): void {
    const index = token.attrIndex(name);
    if (index >= 0) {
        token.attrs?.splice(index, 1);
    }
}

function isAbsoluteHttpUrl(src: string | null): boolean {
    if (!src) {
        return false;
    }

    try {
        const url = new URL(src);
        return url.protocol === "http:" || url.protocol === "https:";
    } catch {
        return false;
    }
}
