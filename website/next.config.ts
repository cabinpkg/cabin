import type { NextConfig } from "next";

const nextConfig: NextConfig = {
    images: {
        remotePatterns: [
            {
                protocol: "https",
                hostname: "vhs.charm.sh",
                pathname: "**",
            },
        ],
    },
};

export default nextConfig;
