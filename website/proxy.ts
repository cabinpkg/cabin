import { type NextRequest, NextResponse } from "next/server";

const IS_DEV = process.env.NODE_ENV === "development";

export function proxy(request: NextRequest) {
    const nonce = Buffer.from(crypto.randomUUID()).toString("base64");

    // TODO: unsafe-inline is needed for NextUI?
    // script-src 'self' 'nonce-${nonce}' 'strict-dynamic' static.cloudflareinsights.com ${
    //     IS_DEV ? "'unsafe-eval'" : ""
    // };
    // style-src 'self' 'nonce-${nonce}';

    const cspHeader = `
    default-src 'self';
    script-src 'self' 'unsafe-inline' static.cloudflareinsights.com ${
        IS_DEV ? "va.vercel-scripts.com 'unsafe-eval'" : ""
    };
    style-src 'self' 'unsafe-inline';
    connect-src 'self' vitals.vercel-insights.com;
    img-src 'self' github.com release-assets.githubusercontent.com blob: data:;
    font-src 'self';
    object-src 'none';
    base-uri 'self';
    form-action 'self';
    frame-ancestors 'none';
    block-all-mixed-content;
    upgrade-insecure-requests;
`;
    // Replace newline characters and spaces
    const contentSecurityPolicyHeaderValue = cspHeader
        .replace(/\s{2,}/g, " ")
        .trim();

    const requestHeaders = new Headers(request.headers);
    requestHeaders.set("x-nonce", nonce);

    requestHeaders.set(
        "Content-Security-Policy",
        contentSecurityPolicyHeaderValue,
    );

    const response = NextResponse.next({
        request: {
            headers: requestHeaders,
        },
    });
    response.headers.set(
        "Content-Security-Policy",
        contentSecurityPolicyHeaderValue,
    );
    response.headers.set("X-Frame-Options", "deny");
    response.headers.set("X-Content-Type-Options", "nosniff");
    response.headers.set("Referrer-Policy", "strict-origin-when-cross-origin");
    response.headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()",
    );

    return response;
}

export const config = {
    matcher: [
        /*
         * Match all request paths except for the ones starting with:
         * - api (API routes)
         * - _next/static (static files)
         * - _next/image (image optimization files)
         * - favicon.ico (favicon file)
         */
        {
            source: "/((?!api|_next/static|_next/image|favicon.ico).*)",
            missing: [
                { type: "header", key: "next-router-prefetch" },
                { type: "header", key: "purpose", value: "prefetch" },
            ],
        },
    ],
};
