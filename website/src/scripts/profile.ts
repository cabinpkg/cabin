// Drives /settings/profile: everything shown here is already in the
// shared auth probe's answer, so no further API calls are made.
import { sharedAuth } from "../lib/account.ts";
import { accountShell } from "../lib/accountShell";
import { loadAvatar } from "../lib/avatar.ts";

const shell = accountShell();
if (shell) {
    sharedAuth().then((auth) => {
        if (auth.state === "signed-out") {
            shell.show("signed-out");
            return;
        }
        if (auth.state === "error") {
            shell.show("error", auth.message);
            return;
        }
        if (auth.state !== "signed-in") {
            return;
        }
        const fields: Array<[string, string]> = [
            ["[data-profile-login]", auth.user.login],
            ["[data-profile-id]", String(auth.user.github_id)],
            ["[data-profile-plan]", auth.user.plan],
        ];
        for (const [selector, text] of fields) {
            const target = shell.root.querySelector(selector);
            if (target instanceof HTMLElement) {
                target.textContent = text;
            }
        }
        // 2x the rendered 48px for high-density displays.
        loadAvatar(shell.root, auth.user, 96);
        shell.show("content");
    });
}
