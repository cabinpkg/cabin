//! Server-rendered HTML for the browser plane: minimal, script-free pages
//! whose every dynamic value goes through [`escape_html`]. The pages fit
//! the CSP the glue sends (`default-src 'none'; style-src 'unsafe-inline'`):
//! one inline `<style>` block, no scripts, no external resources.

use std::fmt::Write as _;

/// Escapes `text` for interpolation into HTML text content and quoted
/// attribute values.
pub fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

const STYLE: &str = "body{font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;\
padding:0 1rem}table{border-collapse:collapse;width:100%}th,td{border:1px solid #cbd5e1;\
padding:.4rem .6rem;text-align:left}code{background:#f1f5f9;padding:.2rem .4rem;\
word-break:break-all}form.inline{display:inline}";

/// Wraps `body_html` (already escaped where dynamic) in the page shell.
fn page(title: &str, body_html: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title><style>{STYLE}</style></head>\
         <body>{body_html}</body></html>",
        title = escape_html(title),
    )
}

/// A heading-plus-paragraph page; both values are escaped here. Used for
/// the 403 sign-in refusal and the small web-plane error pages, so callers
/// pass fixed strings, never request bytes.
pub fn simple_page(title: &str, message: &str) -> String {
    page(
        title,
        &format!(
            "<h1>{title}</h1><p>{message}</p>",
            title = escape_html(title),
            message = escape_html(message),
        ),
    )
}

/// The `/me` usage block: the user's plan, current consumption, and the
/// plan's quota values. `stored_bytes` excludes rejected versions (their
/// bytes are refunded); the per-status counts cover everything the user
/// ever published.
pub struct UsageInfo {
    pub plan: String,
    pub package_count: u64,
    pub stored_bytes: u64,
    pub published_today: u64,
    pub verified_count: u64,
    pub pending_count: u64,
    pub rejected_count: u64,
    pub quotas: crate::quota::PlanQuotas,
}

/// `bytes` for humans: the largest binary unit with one decimal.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    #[allow(clippy::cast_precision_loss)] // display only
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}", unit = UNITS[unit])
    }
}

/// The usage-and-quotas section of the `/me` page.
fn usage_section(usage: &UsageInfo) -> String {
    let quotas = &usage.quotas;
    format!(
        "<h2>Usage</h2><table>\
         <tr><th>Plan</th><td>{plan}</td></tr>\
         <tr><th>Packages</th><td>{package_count} of {max_packages}</td></tr>\
         <tr><th>Stored archives</th><td>{stored} of {max_stored}</td></tr>\
         <tr><th>Versions published today</th><td>{published_today}</td></tr>\
         <tr><th>Versions by verification</th>\
         <td>{verified} verified, {pending} pending, {rejected} rejected</td></tr>\
         </table>\
         <p>Limits: archives up to {max_archive} each; at most\
         \u{20}{max_versions_day} versions per package and {max_new_day} new\
         \u{20}packages per day; publishes refill at {refill} per minute with\
         \u{20}a burst of {burst}.</p>",
        plan = escape_html(&usage.plan),
        package_count = usage.package_count,
        max_packages = quotas.max_packages_total,
        stored = format_bytes(usage.stored_bytes),
        max_stored = format_bytes(quotas.max_total_bytes_per_user),
        published_today = usage.published_today,
        verified = usage.verified_count,
        pending = usage.pending_count,
        rejected = usage.rejected_count,
        max_archive = format_bytes(quotas.max_archive_bytes),
        max_versions_day = quotas.max_versions_per_package_per_day,
        max_new_day = quotas.max_new_packages_per_day,
        refill = quotas.publish_refill_per_minute,
        burst = quotas.publish_burst,
    )
}

/// One token row of the `/me` page, straight from the `tokens` table.
/// Never carries the token hash - the page has no use for it.
pub struct TokenRow {
    pub id: String,
    pub name: String,
    pub scopes: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

/// The `/me` page: the signed-in user's usage and quotas, their tokens,
/// and the create-token form. `csrf` is embedded as a hidden field in
/// every form.
pub fn me_page(login: &str, usage: &UsageInfo, tokens: &[TokenRow], csrf: &str) -> String {
    let mut rows = String::new();
    for token in tokens {
        let action = if token.revoked {
            "revoked".to_owned()
        } else {
            format!(
                "<form class=\"inline\" method=\"post\" action=\"/me/tokens/{id}/revoke\">\
                 <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
                 <button>Revoke</button></form>",
                id = escape_html(&token.id),
                csrf = escape_html(csrf),
            )
        };
        let scopes = if token.scopes.is_empty() {
            "read-only"
        } else {
            &token.scopes
        };
        let _ = write!(
            rows,
            "<tr><td>{name}</td><td>{scopes}</td><td>{created_at}</td>\
             <td>{last_used_at}</td><td>{action}</td></tr>",
            name = escape_html(&token.name),
            scopes = escape_html(scopes),
            created_at = escape_html(&token.created_at),
            last_used_at = escape_html(token.last_used_at.as_deref().unwrap_or("never")),
        );
    }
    let table = if tokens.is_empty() {
        "<p>No tokens yet.</p>".to_owned()
    } else {
        format!(
            "<table><tr><th>Name</th><th>Scopes</th><th>Created</th>\
             <th>Last used</th><th></th></tr>{rows}</table>"
        )
    };
    page(
        "Cabin registry",
        &format!(
            "<h1>Cabin registry</h1>\
             <p>Signed in as <strong>{login}</strong>.</p>\
             {usage}\
             <h2>Tokens</h2>{table}\
             <h2>Create a token</h2>\
             <p>Every token grants read access; the scopes below add the\
             \u{20}matching write routes.</p>\
             <form method=\"post\" action=\"/me/tokens\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <p><label>Name <input name=\"name\" required maxlength=\"64\"></label></p>\
             <p><label><input type=\"checkbox\" name=\"scope_publish\"> publish</label>\
             \u{20}<label><input type=\"checkbox\" name=\"scope_yank\"> yank</label>\
             \u{20}<label><input type=\"checkbox\" name=\"scope_verify\"> verify</label></p>\
             <p><button>Create token</button></p></form>",
            login = escape_html(login),
            usage = usage_section(usage),
            csrf = escape_html(csrf),
        ),
    )
}

/// The one response that ever carries a token's plaintext.
pub fn token_created_page(name: &str, token: &str) -> String {
    page(
        "Token created",
        &format!(
            "<h1>Token created</h1>\
             <p>The token <strong>{name}</strong> is:</p>\
             <p><code>{token}</code></p>\
             <p><strong>Copy it now.</strong> It is shown exactly once;\
             \u{20}the registry stores only its hash.</p>\
             <p><a href=\"/me\">Back to your tokens</a></p>",
            name = escape_html(name),
            token = escape_html(token),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html_covers_the_five_specials() {
        assert_eq!(
            escape_html(r#"<a href="x" onclick='y'>&"#),
            "&lt;a href=&quot;x&quot; onclick=&#39;y&#39;&gt;&amp;"
        );
        assert_eq!(escape_html("plain text"), "plain text");
    }

    fn hostile_row() -> TokenRow {
        TokenRow {
            id: "abc123".to_owned(),
            name: "<script>alert('x')</script>".to_owned(),
            scopes: "publish".to_owned(),
            created_at: "2026-07-09T00:00:00Z".to_owned(),
            last_used_at: None,
            revoked: false,
        }
    }

    fn usage_fixture() -> UsageInfo {
        UsageInfo {
            plan: "free".to_owned(),
            package_count: 3,
            stored_bytes: 12 * 1024 * 1024,
            published_today: 2,
            verified_count: 5,
            pending_count: 1,
            rejected_count: 2,
            quotas: crate::quota::quotas_for_plan("free"),
        }
    }

    #[test]
    fn format_bytes_picks_the_binary_unit() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(16 * 1024 * 1024), "16.0 MiB");
        assert_eq!(format_bytes(256 * 1024 * 1024), "256.0 MiB");
        assert_eq!(format_bytes(9 * 1024 * 1024 * 1024), "9.0 GiB");
    }

    #[test]
    fn me_page_renders_the_usage_and_quota_values() {
        let body = me_page("octocat", &usage_fixture(), &[], "csrf-token");
        assert!(
            body.contains("<tr><th>Plan</th><td>free</td></tr>"),
            "body: {body}"
        );
        assert!(body.contains("<td>3 of 50</td>"), "body: {body}");
        assert!(
            body.contains("<td>12.0 MiB of 256.0 MiB</td>"),
            "body: {body}"
        );
        assert!(
            body.contains("<tr><th>Versions published today</th><td>2</td></tr>"),
            "body: {body}"
        );
        assert!(
            body.contains("<td>5 verified, 1 pending, 2 rejected</td>"),
            "body: {body}"
        );
        // The plan's remaining quota values appear in the limits line.
        assert!(
            body.contains("archives up to 16.0 MiB each"),
            "body: {body}"
        );
        assert!(body.contains("30 versions per package"), "body: {body}");
        assert!(body.contains("5 new packages per day"), "body: {body}");
        assert!(
            body.contains("refill at 1 per minute with a burst of 5"),
            "body: {body}"
        );
        // The create-token form offers all three scopes.
        for scope in ["scope_publish", "scope_yank", "scope_verify"] {
            assert!(
                body.contains(&format!("name=\"{scope}\"")),
                "missing {scope} in: {body}"
            );
        }
    }

    #[test]
    fn me_page_escapes_a_hostile_plan_value() {
        let usage = UsageInfo {
            plan: "<b>free</b>".to_owned(),
            ..usage_fixture()
        };
        let body = me_page("octocat", &usage, &[], "csrf-token");
        assert!(body.contains("&lt;b&gt;free&lt;/b&gt;"), "body: {body}");
        assert!(!body.contains("<b>free</b>"), "body: {body}");
    }

    #[test]
    fn me_page_escapes_token_names() {
        let body = me_page("octocat", &usage_fixture(), &[hostile_row()], "csrf-token");
        assert!(!body.contains("<script>"), "body: {body}");
        assert!(
            body.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"),
            "body: {body}"
        );
    }

    #[test]
    fn me_page_escapes_every_row_cell_and_falls_back_per_column() {
        // Hostile bytes in the cells the fixtures above leave benign, plus
        // the two per-column fallbacks: empty scopes render as read-only,
        // a recorded last_used_at renders (escaped) instead of "never".
        let row = TokenRow {
            scopes: String::new(),
            created_at: "<i>now</i>".to_owned(),
            last_used_at: Some("\"<b>\"".to_owned()),
            ..hostile_row()
        };
        let body = me_page("octocat", &usage_fixture(), &[row], "csrf-token");
        assert!(body.contains("<td>read-only</td>"), "body: {body}");
        assert!(body.contains("&lt;i&gt;now&lt;/i&gt;"), "body: {body}");
        assert!(body.contains("&quot;&lt;b&gt;&quot;"), "body: {body}");
        for raw in ["<i>", "<b>", "\"<"] {
            assert!(!body.contains(raw), "unescaped {raw:?} in: {body}");
        }
        let hostile_scopes = TokenRow {
            scopes: "<em>publish</em>".to_owned(),
            ..hostile_row()
        };
        let body = me_page("octocat", &usage_fixture(), &[hostile_scopes], "csrf-token");
        assert!(
            body.contains("&lt;em&gt;publish&lt;/em&gt;"),
            "body: {body}"
        );
        assert!(!body.contains("<em>"), "body: {body}");
    }

    #[test]
    fn me_page_escapes_the_login_and_embeds_the_csrf_field() {
        let body = me_page("a<b", &usage_fixture(), &[], "csrf-token");
        assert!(body.contains("a&lt;b"), "body: {body}");
        assert!(
            body.contains("<input type=\"hidden\" name=\"csrf\" value=\"csrf-token\">"),
            "body: {body}"
        );
        assert!(body.contains("No tokens yet."), "body: {body}");
    }

    #[test]
    fn me_page_offers_revoke_only_for_active_tokens() {
        let active = hostile_row();
        let revoked = TokenRow {
            revoked: true,
            ..hostile_row()
        };
        let body = me_page(
            "octocat",
            &usage_fixture(),
            &[active, revoked],
            "csrf-token",
        );
        assert!(
            body.contains("action=\"/me/tokens/abc123/revoke\""),
            "body: {body}"
        );
        assert!(body.contains("<td>revoked</td>"), "body: {body}");
        // The hash never reaches the page; there is no column for it.
        assert!(!body.to_lowercase().contains("hash</th>"), "body: {body}");
    }

    #[test]
    fn token_created_page_shows_the_token_once_and_escapes_the_name() {
        let body = token_created_page("a&b", "cabin_0123456789");
        assert!(
            body.contains("<code>cabin_0123456789</code>"),
            "body: {body}"
        );
        assert!(body.contains("a&amp;b"), "body: {body}");
        assert!(body.contains("Copy it now."), "body: {body}");
    }
}
