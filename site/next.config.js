/** @type {import('next').NextConfig} */
const path = require("path");

const securityHeaders = [
    {
        key: "X-Frame-Options",
        value: "deny",
    },
    {
        key: "X-Content-Type-Options",
        value: "nosniff",
    },
    {
        key: "Referrer-Policy",
        value: "strict-origin-when-cross-origin",
    },
];

const nextConfig = {
    reactStrictMode: true,
    async headers() {
        return [
            {
                // Apply these headers to all routes in your application.
                source: "/:path*",
                headers: securityHeaders,
            },
        ];
    },
    webpack: config => {
        config.resolve.alias["~"] = path.resolve(__dirname);
        return config;
    },
};

module.exports = nextConfig;
