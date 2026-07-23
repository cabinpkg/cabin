// node:test suite for the avatar URL derivation (`npm test`).
import assert from "node:assert/strict";
import { test } from "node:test";
import { avatarUrl } from "./avatar.ts";

test("avatarUrl derives the numeric-id GitHub avatar URL", () => {
    assert.equal(
        avatarUrl(
            { github_id: 583231, login: "octocat", quota_class: "default" },
            48,
        ),
        "https://avatars.githubusercontent.com/u/583231?s=48",
    );
});
