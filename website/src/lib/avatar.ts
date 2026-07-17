// GitHub serves every account's avatar at a stable numeric-id URL, so
// the session payload's github_id is enough and the registry never has
// to store or proxy avatars. The URL shape is not a documented API, but
// it is what api.github.com's avatar_url values resolve to.
import type { User } from "./account.ts";

export function avatarUrl(user: User, size: number): string {
    return `https://avatars.githubusercontent.com/u/${user.github_id}?s=${size}`;
}

// Swaps the avatar into a surface marked up with [data-avatar-image] and
// [data-avatar-fallback]. The image replaces the fallback only once it
// has actually loaded, so an unreachable avatar CDN leaves the static
// icon in place instead of a broken-image glyph.
export function loadAvatar(root: HTMLElement, user: User, size: number): void {
    const image = root.querySelector("[data-avatar-image]");
    if (!(image instanceof HTMLImageElement)) {
        return;
    }
    image.addEventListener(
        "load",
        () => {
            const fallback = root.querySelector("[data-avatar-fallback]");
            if (fallback instanceof HTMLElement) {
                fallback.hidden = true;
            }
            image.hidden = false;
        },
        { once: true },
    );
    image.src = avatarUrl(user, size);
}
