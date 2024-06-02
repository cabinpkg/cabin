export default {
    logo: <img src="/dark.svg" alt="Poac logo" width={64} />,
    project: {
        link: "https://github.com/poac-dev/poac",
    },
    docsRepositoryBase: "https://github.com/poac-dev/website/tree/main",
    useNextSeoProps() {
        return {
            titleTemplate: "%s – Poac",
        };
    },
    darkMode: false,
    nextThemes: {
        defaultTheme: "dark",
    },
};
