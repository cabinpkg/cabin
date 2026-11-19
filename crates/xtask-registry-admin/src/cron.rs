//! The check of the registry's cron that a verification pass makes from
//! outside it (`api/docs/runbook.md`, "Alerts").  The cron reports
//! trouble by mail, from the cron: one that has stopped, or whose mail
//! is not going out, cannot report itself.  The admin plane answers
//! what the cron last recorded (`GET /api/v1/admin/cron`) and this
//! decides whether that is trouble.  An answer that did not come, or
//! does not read as one, is trouble too: nothing here reads as healthy
//! by default.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Three of the cron's 15-minute runs.
const LATE_SECONDS: i64 = 45 * 60;

/// The tries after which the cron itself slows an alert's mail down to
/// one a day (`alerts::ATTEMPTS` in `api/`); a notification's fourth
/// try comes three and a half hours after its first.
const FAILED_TRIES: i64 = 4;

/// The admin plane's answer.  A field it lacks fails the read: also
/// one that may be null, which `serde` alone would read as null.
#[derive(Deserialize)]
struct State {
    #[serde(deserialize_with = "Option::deserialize")]
    alerts_checked_seconds: Option<i64>,
    alert_mail_attempts: i64,
    #[serde(deserialize_with = "Option::deserialize")]
    notification_waiting_seconds: Option<i64>,
    notification_attempts: i64,
    #[serde(deserialize_with = "Option::deserialize")]
    notification_sent_seconds: Option<i64>,
}

/// Prints what is wrong with the cron by `answer`, the admin plane's
/// body or why there is none, and fails on any of it.
///
/// # Errors
///
/// When there is a problem.
pub(crate) fn check(answer: Result<String>) -> Result<()> {
    let problems = problems(answer);
    for problem in &problems {
        eprintln!("cron: {problem}");
    }
    if !problems.is_empty() {
        bail!("the registry's cron needs the operator (api/docs/runbook.md, \"Alerts\")");
    }
    println!("cron: the alert pass is recent and no mail is stuck");
    Ok(())
}

/// What is wrong with the cron, by `answer`.  Empty when nothing is.
///
/// A mail that has waited for a pass is trouble only while none goes
/// out: a burst of events waits its turn behind the mails each pass
/// sends, for hours, and that is the pacing at work.
fn problems(answer: Result<String>) -> Vec<String> {
    let state = answer.and_then(|body| {
        serde_json::from_str::<State>(&body).context("the answer is not the cron's state")
    });
    let state = match state {
        Ok(state) => state,
        Err(err) => return vec![format!("the cron's state could not be read: {err:#}")],
    };
    let mut problems = Vec::new();
    match state.alerts_checked_seconds {
        None => problems.push("the alert pass has recorded no run".to_owned()),
        Some(seconds) if seconds > LATE_SECONDS => problems.push(format!(
            "the alert pass last recorded a run {} minutes ago",
            seconds / 60
        )),
        Some(_) => {}
    }
    if state.alert_mail_attempts >= FAILED_TRIES {
        problems.push(format!(
            "an alert's mail has failed {} passes running",
            state.alert_mail_attempts
        ));
    }
    if let Some(waiting) = state.notification_waiting_seconds
        && waiting > LATE_SECONDS
        && state
            .notification_sent_seconds
            .is_none_or(|sent| sent > LATE_SECONDS)
    {
        problems.push(format!(
            "a notification has waited {} minutes for a pass to send it, and no pass is sending",
            waiting / 60
        ));
    }
    if state.notification_attempts >= FAILED_TRIES {
        problems.push(format!(
            "a notification is unsent after {} tries",
            state.notification_attempts
        ));
    }
    problems
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow};

    use super::problems;

    fn state(
        checked: Option<i64>,
        alert_attempts: i64,
        waiting: Option<i64>,
        attempts: i64,
        sent: Option<i64>,
    ) -> Vec<String> {
        problems(Ok(serde_json::json!({
            "alerts_checked_seconds": checked,
            "alert_mail_attempts": alert_attempts,
            "notification_waiting_seconds": waiting,
            "notification_attempts": attempts,
            "notification_sent_seconds": sent,
        })
        .to_string()))
    }

    #[test]
    fn a_recent_pass_with_no_stuck_mail_is_no_problem() {
        assert!(state(Some(2700), 3, None, 3, None).is_empty());
        // A backlog that the passes are sending, however long its tail
        // has waited; and one whose first mail is three passes old.
        assert!(state(Some(0), 0, Some(86_400), 0, Some(2700)).is_empty());
        assert!(state(Some(0), 0, Some(2700), 0, None).is_empty());
    }

    #[test]
    fn each_kind_of_trouble_is_named() {
        assert_eq!(
            state(None, 0, None, 0, None),
            ["the alert pass has recorded no run"]
        );
        assert_eq!(
            state(Some(2701), 0, None, 0, None),
            ["the alert pass last recorded a run 45 minutes ago"]
        );
        assert_eq!(
            state(Some(0), 4, None, 0, None),
            ["an alert's mail has failed 4 passes running"]
        );
        let waiting =
            ["a notification has waited 45 minutes for a pass to send it, and no pass is sending"];
        assert_eq!(state(Some(0), 0, Some(2701), 0, None), waiting);
        assert_eq!(state(Some(0), 0, Some(2701), 0, Some(2701)), waiting);
        assert_eq!(
            state(Some(0), 0, Some(0), 4, Some(0)),
            ["a notification is unsent after 4 tries"]
        );
        assert_eq!(state(None, 9, Some(9000), 9, None).len(), 4);
    }

    #[test]
    fn an_answer_that_does_not_read_is_trouble() {
        const FIELDS: [&str; 5] = [
            "alerts_checked_seconds",
            "alert_mail_attempts",
            "notification_waiting_seconds",
            "notification_attempts",
            "notification_sent_seconds",
        ];
        // The answer with every field but `without`.
        let answer = |without: &str| -> Result<String> {
            let fields: Vec<String> = FIELDS
                .iter()
                .filter(|field| **field != without)
                .map(|field| format!(r#""{field}":0"#))
                .collect();
            Ok(format!("{{{}}}", fields.join(",")))
        };
        assert!(problems(answer("")).is_empty());
        let unread = [
            Err(anyhow!("status code 503")),
            Ok(String::new()),
            answer("").map(|whole| format!("{whole}\0")),
        ]
        .into_iter()
        .chain(FIELDS.map(answer));
        for answer in unread {
            let problems = problems(answer);
            assert_eq!(problems.len(), 1, "{problems:?}");
            assert!(
                problems[0].starts_with("the cron's state could not be read: "),
                "{problems:?}"
            );
        }
    }
}
