import MarkdownIt from "markdown-it";

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

    renderer.renderer.rules.link_open = (...args) => {
        const [tokens, index, options, env, self] = args;
        tokens[index].attrSet("target", "_blank");
        tokens[index].attrSet("rel", "noopener noreferrer");

        return defaultLinkOpen
            ? defaultLinkOpen(tokens, index, options, env, self)
            : self.renderToken(tokens, index, options);
    };

    return renderer;
}
