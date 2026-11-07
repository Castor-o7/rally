//! Keeping a long-running loop going through failures that clear on their own (RAL-86).
//!
//! `rally digest --every` and `rally wake --every` used to exit on their first error, such as the
//! service's `refused (429): too many checks and past-state reads` right after a successful send.
//! Now a failed pass is logged as one JSON line, the loop backs off (honouring the service's
//! `Retry-After` when it sends one), and it carries on. It exits only on an error that won't clear
//! by waiting: bad configuration, no pairing, or a refusal such as a revoked token.

use std::error::Error;
use std::fmt;

use serde_json::{Value, json};

/// The service refused a request: its status, its reason, and how long it asked the client to
/// wait, if it said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub reason: String,
    pub retry_after_ms: Option<u64>,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refused ({}): {}", self.status, self.reason)
    }
}

impl Error for Refused {}

impl Refused {
    /// The refusal a reply that isn't a success stands for, with the service's reason.
    pub fn of(reply: &crate::client::Reply) -> Self {
        let reason = serde_json::from_str::<Value>(&reply.body)
            .ok()
            .and_then(|value| value["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "no reason given".to_owned());
        Self {
            status: reply.status,
            reason,
            retry_after_ms: reply.retry_after_ms,
        }
    }
}

/// An error that waiting won't clear, such as configuration that can't be read.
#[derive(Debug)]
pub struct Fatal(pub String);

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Fatal {}

/// What a failed pass means for the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Exit: waiting won't help.
    Fatal,
    /// Try again later, no sooner than `retry_after_ms` if the service said.
    Transient { retry_after_ms: Option<u64> },
}

/// Sorts an error. A refusal for too many requests (429), a timeout (408) or the service's own
/// trouble (5xx) clears with time; any other refusal (a bad or revoked token, a read-only
/// credential, a bad request) won't. Errors marked [`Fatal`] exit. Everything else, such as the
/// service being unreachable or restarting, or Muster being down, is taken to clear with time.
pub fn classify(error: &(dyn Error + 'static)) -> Kind {
    if error.downcast_ref::<Fatal>().is_some() {
        return Kind::Fatal;
    }
    if let Some(refused) = error.downcast_ref::<Refused>() {
        return match refused.status {
            408 | 429 | 500..=599 => Kind::Transient {
                retry_after_ms: refused.retry_after_ms,
            },
            _ => Kind::Fatal,
        };
    }
    Kind::Transient {
        retry_after_ms: None,
    }
}

/// The longest the loop waits after a failure, whatever the service asks.
pub const MAX_WAIT_MS: u64 = 60 * 60 * 1000;

/// How long to wait before the next pass, after `failures` failures in a row (0 after a success):
/// `every` after a success; after a failure, `every` doubled per failure in a row up to 16 times
/// `every`, and never sooner than the service asked; at most an hour.
pub fn wait_ms(every_ms: u64, failures: u32, retry_after_ms: Option<u64>) -> u64 {
    if failures == 0 {
        return every_ms;
    }
    let doubled = every_ms.saturating_mul(1u64 << (failures - 1).min(4));
    doubled.max(retry_after_ms.unwrap_or(0)).min(MAX_WAIT_MS)
}

/// The JSON line logged for a failed pass.
pub fn failure_line(error: &str, failures: u32, retry_in_ms: u64, kind: Kind) -> Value {
    let mut line = json!({
        "pass": "failed", "error": error, "failures_in_a_row": failures,
        "retry_in_ms": retry_in_ms,
    });
    if let Kind::Transient {
        retry_after_ms: Some(asked),
    } = kind
    {
        line["retry_after_ms"] = json!(asked);
    }
    line
}

/// Runs `pass` every `every_ms` until a fatal error, which it returns. Before each pass,
/// `preflight` checks what must hold for a pass to be worth trying (configuration, the pairing);
/// its error is fatal. Each report goes to `out`, and so does each failed pass, as one JSON line.
/// `sleep` waits; tests pass one that records instead.
pub fn run(
    every_ms: u64,
    mut preflight: impl FnMut() -> Result<(), Box<dyn Error>>,
    mut pass: impl FnMut() -> Result<String, Box<dyn Error>>,
    mut out: impl FnMut(&str),
    mut sleep: impl FnMut(u64),
) -> Box<dyn Error> {
    let mut failures = 0u32;
    loop {
        if let Err(error) = preflight() {
            return Box::new(Fatal(error.to_string()));
        }
        let wait = match pass() {
            Ok(report) => {
                failures = 0;
                out(&report);
                wait_ms(every_ms, 0, None)
            }
            Err(error) => {
                let kind = classify(error.as_ref());
                if kind == Kind::Fatal {
                    return error;
                }
                failures = failures.saturating_add(1);
                let Kind::Transient { retry_after_ms } = kind else {
                    unreachable!()
                };
                let wait = wait_ms(every_ms, failures, retry_after_ms);
                out(&failure_line(&error.to_string(), failures, wait, kind).to_string());
                wait
            }
        };
        sleep(wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn refused(status: u16, retry_after_ms: Option<u64>) -> Box<dyn Error> {
        Box::new(Refused {
            status,
            reason: "too many checks and past-state reads".into(),
            retry_after_ms,
        })
    }

    #[test]
    fn errors_that_clear_with_time_are_told_from_ones_that_dont() {
        let transient = |e: Box<dyn Error>| matches!(classify(e.as_ref()), Kind::Transient { .. });
        assert!(transient(refused(429, None)));
        assert!(transient(refused(503, None)));
        assert!(transient(refused(408, None)));
        assert!(transient("connection refused".into()));
        assert!(!transient(refused(401, None)));
        assert!(!transient(refused(403, None)));
        assert!(!transient(refused(400, None)));
        assert!(!transient(Box::new(Fatal("no pairing".into()))));
        assert_eq!(
            classify(refused(429, Some(7_000)).as_ref()),
            Kind::Transient {
                retry_after_ms: Some(7_000)
            }
        );
    }

    #[test]
    fn the_wait_doubles_is_capped_and_honours_retry_after() {
        assert_eq!(wait_ms(30_000, 0, None), 30_000);
        assert_eq!(wait_ms(30_000, 1, None), 30_000);
        assert_eq!(wait_ms(30_000, 2, None), 60_000);
        assert_eq!(wait_ms(30_000, 5, None), 480_000);
        assert_eq!(wait_ms(30_000, 50, None), 480_000, "16 times every at most");
        assert_eq!(wait_ms(30_000, 1, Some(90_000)), 90_000);
        assert_eq!(wait_ms(30_000, 1, Some(10 * MAX_WAIT_MS)), MAX_WAIT_MS);
    }

    /// The watchman's case: a 429 right after a successful pass is logged and waited out, and the
    /// next pass runs; the loop ends only on a fatal error.
    #[test]
    fn a_refusal_is_logged_and_waited_out_and_the_loop_keeps_going() {
        let replies: RefCell<Vec<Result<String, Box<dyn Error>>>> = RefCell::new(vec![
            Ok("first".into()),
            Err(refused(429, Some(45_000))),
            Err(refused(429, None)),
            Ok("second".into()),
            Err(refused(401, None)),
        ]);
        let lines = RefCell::new(Vec::new());
        let waits = RefCell::new(Vec::new());
        let error = run(
            30_000,
            || Ok(()),
            || replies.borrow_mut().remove(0),
            |line| lines.borrow_mut().push(line.to_owned()),
            |ms| waits.borrow_mut().push(ms),
        );
        assert_eq!(
            error.to_string(),
            "refused (401): too many checks and past-state reads"
        );
        assert_eq!(*waits.borrow(), [30_000, 45_000, 60_000, 30_000]);
        let lines = lines.borrow();
        assert_eq!(lines[0], "first");
        let failed: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(failed["pass"], "failed");
        assert_eq!(failed["retry_after_ms"], 45_000);
        assert_eq!(failed["failures_in_a_row"], 1);
        let again: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(again["failures_in_a_row"], 2);
        assert_eq!(lines[3], "second");
        assert_eq!(
            lines.len(),
            4,
            "the fatal error is returned, not logged as a pass"
        );
    }

    #[test]
    fn a_failed_preflight_ends_the_loop_before_any_pass() {
        let passes = RefCell::new(0);
        let error = run(
            1,
            || Err("no pairing at /x: run `rally pair`".into()),
            || {
                *passes.borrow_mut() += 1;
                Ok(String::new())
            },
            |_| {},
            |_| {},
        );
        assert!(error.to_string().contains("no pairing"));
        assert_eq!(*passes.borrow(), 0);
    }
}
