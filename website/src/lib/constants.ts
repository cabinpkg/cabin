export const SITE_URL = "https://cabinpkg.com";
export const SITE_NAME = "Cabin";
export const SITE_DESCRIPTION =
    "Modern, intuitive, and lightning-fast package manager and build system for C and C++ developers.";

export const DEFAULT_SEARCH_PAGE = 1;
export const DEFAULT_SEARCH_PER_PAGE = 20;
export const SEARCH_PATH = "/search";

export const EXTERNAL_URLS = {
    docs: "https://docs.cabinpkg.com",
    install: "https://docs.cabinpkg.com/installation/",
    githubOrg: "https://github.com/cabinpkg",
    sponsor: "https://github.com/sponsors/ken-matsui",
    author: "https://github.com/ken-matsui",
    demoGif:
        "https://github.com/cabinpkg/cabin/releases/latest/download/demo.gif",
} as const;

export const NAV_LINKS = {
    docs: {
        label: "Docs",
        href: EXTERNAL_URLS.docs,
    },
    github: {
        label: "GitHub Repository",
        href: EXTERNAL_URLS.githubOrg,
    },
    sponsor: {
        label: "Sponsor",
        href: EXTERNAL_URLS.sponsor,
    },
} as const;
