// Drives /settings/profile: everything shown here is already in the
// shared auth probe's answer, so no further API calls are made.
import { bootAccountShell } from "../lib/accountShell";
import { loadAvatar } from "../lib/avatar.ts";

bootAccountShell((shell, user) => {
    const fields: Array<[string, string]> = [
        ["[data-profile-login]", user.login],
        ["[data-profile-id]", String(user.github_id)],
        ["[data-profile-quota-class]", user.quota_class],
    ];
    for (const [selector, text] of fields) {
        const target = shell.root.querySelector(selector);
        if (target instanceof HTMLElement) {
            target.textContent = text;
        }
    }
    // 2x the rendered 48px for high-density displays.
    loadAvatar(shell.root, user, 96);
    shell.show("content");
});
