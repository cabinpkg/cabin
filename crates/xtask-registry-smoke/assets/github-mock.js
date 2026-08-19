const crypto = require("crypto");
const http = require("http");
const port = process.argv[2];
// The Actions OIDC issuer for the verdict endpoint: one keypair per
// run, served as the JWKS the worker's GITHUB_JWKS_URL override points
// at, with a local-only mint endpoint in place of Actions' token URL.
// Claim defaults mirror the production VERIFIER_* pins in
// registry/wrangler.jsonc so the deployed pin values are what the run
// exercises; the negative legs override individual claims.
const { publicKey, privateKey } = crypto.generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: "jwk" }), kid: "smoke-oidc", alg: "RS256", use: "sig" };
const b64url = (data) => Buffer.from(data).toString("base64url");
const mintJwt = (overrides) => {
  const now = Math.floor(Date.now() / 1000);
  const claims = {
    iss: "https://token.actions.githubusercontent.com",
    aud: "cabinpkg.com/verifier",
    jti: crypto.randomUUID(),
    iat: now,
    exp: now + 300,
    repository_owner_id: "35998702",
    repository_id: "119684778",
    workflow_ref: "cabinpkg/cabin/.github/workflows/registry-verify.yml@refs/heads/main",
    ref: "refs/heads/main",
    ...overrides,
  };
  const signing = `${b64url(JSON.stringify({ alg: "RS256", typ: "JWT", kid: "smoke-oidc" }))}.${b64url(JSON.stringify(claims))}`;
  const signature = crypto.sign("sha256", Buffer.from(signing), privateKey).toString("base64url");
  return `${signing}.${signature}`;
};
const api = {
  "/user": { id: 0, login: "Smoke" },
  "/users/smoke": { id: 0, login: "smoke", type: "User" },
  "/users/smokeorg": { id: 7280970, login: "smokeorg", type: "Organization" },
  "/users/denyorg": { id: 555, login: "denyorg", type: "Organization" },
  "/users/imposterorg": { id: 666, login: "imposterorg", type: "Organization" },
  "/users/swaporg": { id: 777, login: "swaporg", type: "Organization" },
  "/orgs/smokeorg/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 7280970, login: "smokeorg" },
  },
  "/orgs/denyorg/memberships/Smoke": {
    state: "active", role: "member",
    user: { id: 0, login: "Smoke" },
    organization: { id: 555, login: "denyorg" },
  },
  "/orgs/imposterorg/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 999, login: "Smoke" },
    organization: { id: 666, login: "imposterorg" },
  },
  "/orgs/swaporg/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 778, login: "swaporg" },
  },
  "/users/statedrift": { id: 888, login: "statedrift", type: "Organization" },
  "/orgs/statedrift/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 888, login: "statedrift" },
  },
  // Fully grantable like statedrift, so their refusals can only be
  // the name-fidelity checks: 'core' is reserved vocabulary, and
  // 'sm0keorg' skeleton-folds to the claimed 'smokeorg'.
  "/users/core": { id: 900, login: "core", type: "Organization" },
  "/orgs/core/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 900, login: "core" },
  },
  "/users/sm0keorg": { id: 901, login: "sm0keorg", type: "Organization" },
  "/orgs/sm0keorg/memberships/Smoke": {
    state: "active", role: "admin",
    user: { id: 0, login: "Smoke" },
    organization: { id: 901, login: "sm0keorg" },
  },
};
// POST /__drift/on makes /users/smoke name a different account than
// /user, so the self-claim's id-equality refusal can be exercised and
// then reverted within one run.
let drift = false;
// POST /__signin/<mode> swaps /user between the seeded account and the
// sign-up age gate's profiles; "off" restores the default. The young
// created_at is relative to now (the gate compares against the clock),
// trimmed to GitHub's second-precision shape - the registry's strict
// parser rejects toISOString's milliseconds.
let signin = "off";
const signinUsers = () => {
  const young = new Date(Date.now() - 5 * 86400000)
    .toISOString()
    .replace(/\.\d+Z$/, "Z");
  return {
    "existing-young": { id: 0, login: "Smoke", created_at: young },
    "fresh-young": { id: 3, login: "fresh", created_at: young },
    "fresh-old": { id: 4, login: "veteran", created_at: "2008-01-14T04:33:35Z" },
  };
};
http.createServer((req, res) => {
  res.setHeader("content-type", "application/json");
  if (req.method === "POST" && (req.url === "/__drift/on" || req.url === "/__drift/off")) {
    drift = req.url === "/__drift/on";
    res.end("{}");
  } else if (req.method === "POST" && req.url.startsWith("/__signin/")) {
    const mode = req.url.slice("/__signin/".length);
    if (mode !== "off" && !signinUsers()[mode]) {
      res.statusCode = 404;
      res.end(JSON.stringify({ message: "unknown signin mode" }));
      return;
    }
    signin = mode;
    res.end("{}");
  } else if (req.method === "GET" && req.url === "/.well-known/jwks") {
    // Unauthenticated, like GitHub's real JWKS.
    res.end(JSON.stringify({ keys: [jwk] }));
  } else if (req.method === "POST" && req.url === "/__oidc/token") {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      res.end(JSON.stringify({ jwt: mintJwt(body ? JSON.parse(body) : {}) }));
    });
  } else if (req.method === "POST" && req.url === "/login/oauth/access_token") {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const redirect = new URLSearchParams(body).get("redirect_uri");
      const registered = ["https://cabinpkg.com/callback", "https://cabinpkg.com/callback/claim"];
      if (!registered.includes(redirect)) {
        res.statusCode = 400;
        res.end(JSON.stringify({ error: "redirect_uri_mismatch" }));
        return;
      }
      res.end(JSON.stringify({ access_token: "gho_smoke", token_type: "bearer" }));
    });
  } else if (req.method === "GET" && api[req.url]) {
    if (!/^Bearer gho_smoke$/.test(req.headers.authorization || "")) {
      res.statusCode = 401;
      res.end(JSON.stringify({ message: "Requires authentication" }));
      return;
    }
    if (req.url === "/users/smoke" && drift) {
      res.end(JSON.stringify({ id: 999, login: "smoke", type: "User" }));
      return;
    }
    if (req.url === "/user" && signin !== "off") {
      res.end(JSON.stringify(signinUsers()[signin]));
      return;
    }
    res.end(JSON.stringify(api[req.url]));
  } else {
    res.statusCode = 404;
    res.end(JSON.stringify({ message: "Not Found" }));
  }
}).listen(port, "127.0.0.1");
