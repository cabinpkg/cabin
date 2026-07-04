import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";

// Canonical docs live at the repository root under `docs/`, not inside this
// Astro project.  The glob `base` is resolved relative to the Astro project
// root (`website/`), so `../docs` points at `<repo-root>/docs`.
//
// The patterns match the flat, top-level Markdown pages plus the
// `docs/design/` tree (normative design documents) - exactly the published
// documentation.  They structurally skip the nested, git-ignored
// `docs/superpowers/` agent workspace, so the route set stays deterministic
// on a clean CI checkout.
const docs = defineCollection({
    loader: glob({ pattern: ["*.md", "design/**/*.md"], base: "../docs" }),
});

export const collections = { docs };
