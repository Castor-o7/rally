//! Recognizes text that looks like a secret, so the ledger can refuse it (RAL-17). An append-only
//! ledger can't forget a record, so a secret written into one stays readable to everyone who can
//! read the ledger.
//!
//! This only recognizes the formats below. A secret in any other form gets through, so the check is
//! a safety net, not a guarantee.

/// What a secret looks like, as a name for the refusal. Never includes the text itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    GitHubToken,
    AwsAccessKey,
    SlackToken,
    GoogleApiKey,
    AnthropicKey,
    OpenAiKey,
    Jwt,
    PrivateKey,
    /// A long hex or base64 run right after a word like `token`, `key`, `secret` or `password`.
    LabelledSecret,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHubToken => "GitHub token",
            Self::AwsAccessKey => "AWS access key",
            Self::SlackToken => "Slack token",
            Self::GoogleApiKey => "Google API key",
            Self::AnthropicKey => "Anthropic API key",
            Self::OpenAiKey => "OpenAI API key",
            Self::Jwt => "JSON web token",
            Self::PrivateKey => "private key",
            Self::LabelledSecret => "labelled secret",
        }
    }
}

/// A prefix, the characters allowed after it, the fewest of them a real one has, and its format.
type Prefixed = (&'static str, fn(char) -> bool, usize, Format);

const PREFIXED: &[Prefixed] = &[
    ("github_pat_", word, 22, Format::GitHubToken),
    ("ghp_", alnum, 36, Format::GitHubToken),
    ("gho_", alnum, 36, Format::GitHubToken),
    ("ghu_", alnum, 36, Format::GitHubToken),
    ("ghs_", alnum, 36, Format::GitHubToken),
    ("ghr_", alnum, 36, Format::GitHubToken),
    ("AKIA", upper_digit, 16, Format::AwsAccessKey),
    ("ASIA", upper_digit, 16, Format::AwsAccessKey),
    ("xoxb-", slug, 10, Format::SlackToken),
    ("xoxp-", slug, 10, Format::SlackToken),
    ("xoxa-", slug, 10, Format::SlackToken),
    ("xoxr-", slug, 10, Format::SlackToken),
    ("xoxs-", slug, 10, Format::SlackToken),
    ("AIza", url_safe, 35, Format::GoogleApiKey),
    // Before `sk-`, which it also starts with.
    ("sk-ant-", url_safe, 20, Format::AnthropicKey),
    ("sk-", url_safe, 20, Format::OpenAiKey),
];

/// Words that, right before a long random-looking run, mark it as a secret.
const LABELS: &[&str] = &["token", "key", "secret", "password", "passwd"];

/// How long a labelled run has to be to count.
const LABELLED_MIN: usize = 32;

fn alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}
fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
fn upper_digit(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}
fn slug(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}
fn url_safe(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}
fn base64ish(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
}

/// The first secret-looking thing in `text`, if any.
pub fn find(text: &str) -> Option<Format> {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        return Some(Format::PrivateKey);
    }
    for (start, _) in text.char_indices() {
        // Only where a token can start, so `mask-...` isn't read as `sk-...`.
        let before = text[..start].chars().next_back();
        if before.is_some_and(url_safe) {
            continue;
        }
        let rest = &text[start..];
        if let Some(format) = prefixed(rest) {
            return Some(format);
        }
        if is_jwt(rest) {
            return Some(Format::Jwt);
        }
        if labelled(rest) {
            return Some(Format::LabelledSecret);
        }
    }
    None
}

fn prefixed(rest: &str) -> Option<Format> {
    PREFIXED.iter().find_map(|&(prefix, allowed, min, format)| {
        let body = rest.strip_prefix(prefix)?;
        (body.chars().take_while(|&c| allowed(c)).count() >= min).then_some(format)
    })
}

/// Three dot-separated base64url parts, the first a JSON header (`eyJ`), each a usable length.
fn is_jwt(rest: &str) -> bool {
    if !rest.starts_with("eyJ") {
        return false;
    }
    let token: String = rest
        .chars()
        .take_while(|&c| url_safe(c) || c == '.')
        .collect();
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 3 && parts[0].len() >= 10 && parts[1].len() >= 10 && parts[2].len() >= 10
}

/// A label word, such as `api_key` or `TOKEN`, then a separator, then a long run with a digit in it.
fn labelled(rest: &str) -> bool {
    let lower = rest.to_ascii_lowercase();
    // A label can carry a prefix like `api_` or `secret_`, so look for one inside the first word.
    let name_len = lower.chars().take_while(|&c| word(c) || c == '-').count();
    let name = &lower[..name_len];
    if !LABELS.iter().any(|label| name.ends_with(label)) {
        return false;
    }
    let after = &rest[name_len..];
    let separator = after
        .chars()
        .take_while(|c| matches!(c, ' ' | ':' | '=' | '"' | '\''))
        .count();
    if separator == 0 || separator > 4 {
        return false;
    }
    let run: String = after[separator..]
        .chars()
        .take_while(|&c| base64ish(c))
        .collect();
    run.len() >= LABELLED_MIN && run.chars().any(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test secrets are built at run time, so that no secret-shaped string is in the source.
    fn rep(c: char, n: usize) -> String {
        std::iter::repeat_n(c, n).collect()
    }

    /// Each format, with a near miss that must pass.
    fn cases() -> Vec<(Format, String, String)> {
        let hex40 = "0123456789abcdef".repeat(3)[..40].to_string();
        let mixed = format!("Ab9{}", rep('x', 33));
        vec![
            (
                Format::GitHubToken,
                format!("token is ghp_{}", rep('a', 36)),
                format!("ghp_{} is too short", rep('a', 20)),
            ),
            (
                Format::GitHubToken,
                format!("github_pat_{}", rep('B', 30)),
                "see github_pat_ docs".to_string(),
            ),
            (
                Format::AwsAccessKey,
                format!("AKIA{}", rep('Q', 16)),
                format!("AKIA{} is short", rep('Q', 8)),
            ),
            (
                Format::SlackToken,
                format!("xoxb-{}", rep('7', 20)),
                "the xoxb- prefix".to_string(),
            ),
            (
                Format::GoogleApiKey,
                format!("AIza{}", rep('z', 35)),
                "AIza is a prefix".to_string(),
            ),
            (
                Format::AnthropicKey,
                format!("sk-ant-{}", rep('k', 40)),
                "sk-ant-short".to_string(),
            ),
            (
                Format::OpenAiKey,
                format!("key sk-{}", rep('m', 40)),
                format!("risk-{}", rep('m', 40)),
            ),
            (
                Format::Jwt,
                format!("eyJ{}.eyJ{}.{}", rep('h', 12), rep('p', 12), rep('s', 12)),
                "eyJhbGciOiJ is only a header".to_string(),
            ),
            (
                Format::PrivateKey,
                format!("-----BEGIN RSA PRIVATE KEY-----\n{}", rep('M', 20)),
                "-----BEGIN PUBLIC KEY-----".to_string(),
            ),
            (
                Format::LabelledSecret,
                format!("api_key = {mixed}"),
                format!("commit {hex40}"),
            ),
            (
                Format::LabelledSecret,
                format!("PASSWORD: \"{hex40}\""),
                format!("the key fix is in {hex40}"),
            ),
        ]
    }

    #[test]
    fn each_format_is_found_and_its_near_miss_passes() {
        for (format, secret, near_miss) in cases() {
            assert_eq!(find(&secret), Some(format), "{format:?} wasn't found");
            assert_eq!(
                find(&near_miss),
                None,
                "{format:?} near miss was refused: {near_miss}"
            );
        }
    }

    #[test]
    fn ordinary_evidence_passes() {
        for text in [
            "Castor-o7/rally #57 at 0123456789abcdef0123456789abcdef01234567",
            "tested: cargo test, 54 passed; not tested: a real restart",
            "blocked on the operator's key decision",
            "monkey_business: 0123456789abcdef0123456789abcdef",
            "https://github.com/Castor-o7/rally/pull/57",
        ] {
            assert_eq!(find(text), None, "{text}");
        }
    }
}
