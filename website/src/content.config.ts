import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";

// Canonical docs live at the repository root under `docs/`, not inside this
// Astro project.  The glob `base` is resolved relative to the Astro project
// root (`website/`), so `../docs` points at `<repo-root>/docs`.
//
// `pattern: "*.md"` matches only the flat, top-level Markdown pages - exactly
// the published documentation.  It structurally skips the nested,
// git-ignored `docs/superpowers/` agent workspace, so the route set stays
// deterministic on a clean CI checkout.
const docs = defineCollection({
    loader: glob({ pattern: "*.md", base: "../docs" }),
});

export const collections = { docs };
