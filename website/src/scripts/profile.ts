// Drives /settings/profile: everything shown here is already in the
// shared auth probe's answer, so no further API calls are made.
import { bootAccountShell, setText } from "../lib/accountShell";
import { loadAvatar } from "../lib/avatar.ts";

bootAccountShell((shell, user) => {
    setText(shell.root, "[data-profile-login]", user.login);
    setText(shell.root, "[data-profile-id]", String(user.github_id));
    setText(shell.root, "[data-profile-quota-class]", user.quota_class);
    // 2x the rendered 48px for high-density displays.
    loadAvatar(shell.root, user, 96);
    shell.show("content");
});
