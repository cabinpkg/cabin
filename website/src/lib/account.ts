// Client for the registry's session-cookie JSON user API, mounted on this
// site's origin under /api/v1/user (see registry/docs/architecture.md in
// the repository). Deliberately dependency-free: the node:test suite in
// account.test.ts exercises it against a mocked fetch.
//
// Every call rides the session cookie (`credentials: "same-origin"`);
// mutations carry the stateless CSRF pair the Worker requires. Responses
// use the crates.io-style error envelope {"errors":[{"detail":...}]} whose
// detail strings are already human-readable - notices reuse them verbatim.
// Nothing here ever touches localStorage: auth state lives in the cookie,
// and payloads live only in the DOM of the page that fetched them.

export interface User {
    github_id: number;
    login: string;
    plan: string;
}

export interface Quotas {
    max_archive_bytes: number;
    max_total_bytes_per_user: number;
    max_new_packages_per_day: number;
    max_packages_total: number;
    max_versions_per_package_per_day: number;
    publish_burst: number;
    publish_refill_per_minute: number;
}

export interface Usage {
    plan: string;
    package_count: number;
    stored_bytes: number;
    published_today: number;
    versions: { verified: number; pending: number; rejected: number };
    quotas: Quotas;
}

export interface PackageVersion {
    version: string;
    verification: "pending" | "verified" | "rejected";
    yanked: boolean;
    published_at: string;
    // Approximate served-download count; always 0 for pending and
    // rejected versions.
    downloads: number;
}

export interface AccountPackage {
    name: string;
    versions: PackageVersion[];
}

export interface TokenInfo {
    id: string;
    name: string;
    scopes: string[];
    created_at: string;
    last_used_at: string | null;
    revoked: boolean;
}

/// The one payload that ever carries a plaintext token.
export interface CreatedToken {
    id: string;
    name: string;
    scopes: string[];
    token: string;
}

export const TOKEN_SCOPES = ["publish", "yank", "verify"] as const;

export type FetchLike = (
    input: string,
    init?: RequestInit,
) => Promise<Response>;

export type ApiResult<T> =
    | { ok: true; data: T }
    // status 0 means the fetch itself failed (registry unreachable).
    | { ok: false; status: number; message: string };

// The auth module's state machine: pages start in "loading" (the static
// HTML state), and resolveAuth() settles into exactly one of the others.
export type AuthState =
    | { state: "loading" }
    | { state: "signed-in"; user: User }
    | { state: "signed-out" }
    | { state: "error"; message: string };

const UNREACHABLE_MESSAGE = "the registry could not be reached";

// Renders a non-2xx response into a human-readable notice, reusing the
// API's own detail string when the envelope carries one.
function errorMessage(status: number, body: unknown): string {
    if (typeof body === "object" && body !== null && "errors" in body) {
        const errors = (body as { errors: unknown }).errors;
        if (Array.isArray(errors)) {
            const detail = (errors[0] as { detail?: unknown } | undefined)
                ?.detail;
            if (typeof detail === "string" && detail !== "") {
                return detail;
            }
        }
    }
    return `the registry answered ${status}`;
}

async function request<T>(
    fetchFn: FetchLike,
    path: string,
    init?: RequestInit,
): Promise<ApiResult<T>> {
    let response: Response;
    try {
        response = await fetchFn(path, {
            credentials: "same-origin",
            ...init,
        });
    } catch {
        return { ok: false, status: 0, message: UNREACHABLE_MESSAGE };
    }
    let body: unknown = null;
    try {
        body = await response.json();
    } catch {
        // A non-JSON body only matters for the error message fallback.
    }
    if (!response.ok) {
        return {
            ok: false,
            status: response.status,
            message: errorMessage(response.status, body),
        };
    }
    return { ok: true, data: body as T };
}

function mutation(body?: unknown): RequestInit {
    return {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
            "X-CSRF-Protection": "1",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
    };
}

export function getUser(fetchFn: FetchLike): Promise<ApiResult<User>> {
    return request(fetchFn, "/api/v1/user");
}

export function getUsage(fetchFn: FetchLike): Promise<ApiResult<Usage>> {
    return request(fetchFn, "/api/v1/user/usage");
}

export function getPackages(
    fetchFn: FetchLike,
): Promise<ApiResult<{ packages: AccountPackage[] }>> {
    return request(fetchFn, "/api/v1/user/packages");
}

export function getTokens(
    fetchFn: FetchLike,
): Promise<ApiResult<{ tokens: TokenInfo[] }>> {
    return request(fetchFn, "/api/v1/user/tokens");
}

export function createToken(
    fetchFn: FetchLike,
    name: string,
    scopes: string[],
): Promise<ApiResult<CreatedToken>> {
    return request(fetchFn, "/api/v1/user/tokens", mutation({ name, scopes }));
}

// Signing out is a session-plane mutation: the cookie is HttpOnly, so
// only the response's Set-Cookie can clear it.
export function signOut(
    fetchFn: FetchLike,
): Promise<ApiResult<{ ok: boolean }>> {
    return request(fetchFn, "/api/v1/user/logout", mutation({}));
}

export function revokeToken(
    fetchFn: FetchLike,
    id: string,
): Promise<ApiResult<{ ok: boolean }>> {
    // The empty object keeps the declared JSON content type truthful.
    return request(
        fetchFn,
        `/api/v1/user/tokens/${encodeURIComponent(id)}/revoke`,
        mutation({}),
    );
}

// Settles the auth state machine: 200 -> signed-in, 401 -> signed-out,
// anything else (the registry down or broken) -> error. Callers render
// "loading" until this resolves.
export async function resolveAuth(fetchFn: FetchLike): Promise<AuthState> {
    const result = await getUser(fetchFn);
    if (result.ok) {
        return { state: "signed-in", user: result.data };
    }
    if (result.status === 401) {
        return { state: "signed-out" };
    }
    return { state: "error", message: result.message };
}

// How every account operation settles for the page that ran it: a 401
// means the session ended, and the whole page must fall back to the
// signed-out state rather than keep stale account data on screen.
export type Outcome<T> =
    | { state: "done"; data: T }
    | { state: "signed-out" }
    | { state: "failed"; message: string };

export function asOutcome<T>(result: ApiResult<T>): Outcome<T> {
    if (result.ok) {
        return { state: "done", data: result.data };
    }
    if (result.status === 401) {
        return { state: "signed-out" };
    }
    return { state: "failed", message: result.message };
}

// The one auth probe a page load makes: the header enhancer and the
// account pages share this promise, so /api/v1/user is fetched once.
let sharedAuthState: Promise<AuthState> | undefined;

export function sharedAuth(): Promise<AuthState> {
    sharedAuthState ??= resolveAuth((input, init) => fetch(input, init));
    return sharedAuthState;
}
