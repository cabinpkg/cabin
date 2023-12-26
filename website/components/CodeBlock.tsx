import { Code, useColorMode } from "@chakra-ui/react";
import type {
    CodeComponent,
    ReactMarkdownNames,
} from "react-markdown/lib/ast-to-react";
import { Prism as SyntaxHighlighter } from "react-syntax-highlighter";
// @ts-ignore
import oneDark from "react-syntax-highlighter/dist/cjs/styles/prism/one-dark";
// @ts-ignore
import oneLight from "react-syntax-highlighter/dist/cjs/styles/prism/one-light";

export const CodeBlock: CodeComponent | ReactMarkdownNames = ({
    inline,
    className,
    children,
    ...props
}) => {
    const { colorMode } = useColorMode();
    const match = /language-(\w+)/.exec(className || "");

    return !inline && match ? (
        <SyntaxHighlighter
            language={match[1]}
            PreTag="div"
            {...props}
            style={colorMode === "light" ? oneLight : oneDark}
        >
            {String(children).replace(/\n$/, "")}
        </SyntaxHighlighter>
    ) : (
        <Code>{children}</Code>
    );
};
