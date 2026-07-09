-- Canonical registry state. D1 is the source of truth; R2 only holds
-- immutable, content-addressed archive blobs (blobs/sha256/<checksum-hex>).

CREATE TABLE users (
    github_id INTEGER PRIMARY KEY,
    login TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE tokens (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at TEXT
);

CREATE TABLE packages (
    name TEXT PRIMARY KEY,
    created_at TEXT NOT NULL
);

CREATE TABLE versions (
    name TEXT NOT NULL REFERENCES packages,
    version TEXT NOT NULL,
    checksum TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    yanked INTEGER NOT NULL DEFAULT 0,
    published_at TEXT NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO meta (key, value) VALUES ('registry_generation', '1');
