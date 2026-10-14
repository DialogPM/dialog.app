use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Whether `s` is an id as made by `new_id` (a UUID). Ids from other clients end up in file
/// names and queries: anything else is rejected.
pub fn is_id(s: &str) -> bool {
    s.len() == 36 && uuid::Uuid::parse_str(s).is_ok()
}

/// Formats a 16-digit account number as `1234 5678 9012 3456`.
pub fn format_number(number: &str) -> String {
    number
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalizes user input into a 16-digit account number.
pub fn normalize_number(input: &str) -> Option<String> {
    let digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    let other = input.chars().any(|c| !c.is_ascii_digit() && !c.is_whitespace() && c != '-');
    (digits.len() == 16 && !other).then_some(digits)
}

/// Normalizes a server URL: trims, adds https:// when missing, removes trailing slashes.
pub fn normalize_server_url(input: &str) -> Option<String> {
    let s = input.trim().trim_end_matches('/');
    if s.is_empty() {
        return None;
    }
    let with_scheme =
        if s.starts_with("http://") || s.starts_with("https://") { s.to_string() } else { format!("https://{s}") };
    let url = url::Url::parse(&with_scheme).ok()?;
    url.host_str()?;
    // Plain HTTP would expose the session token and all metadata on the network: only for a
    // server on this machine or the local network (development, testing).
    if url.scheme() == "http" && !is_local_host(&url) {
        return None;
    }
    Some(with_scheme)
}

/// Minimum length of a new account password.
pub const MIN_PASSWORD_LEN: usize = 12;
/// From this length on, a passphrase needs no character mix.
pub const PASSPHRASE_LEN: usize = 20;
const COMMON_PASSWORDS: &[&str] = &[
    "password", "parola", "passwort", "motdepasse", "qwerty", "qwertz", "azerty", "asdf", "letmein", "welcome",
    "admin", "chatapp", "iloveyou", "abc", "abcdef", "monkey", "dragon", "football", "secret", "test",
];

/// What a new password lacks: "length", "mix" (3 of lowercase, uppercase, digits, symbols, or a
/// passphrase of 20+ characters) and/or "common" (a well-known password, digits/symbols added).
/// Empty means it is accepted. The apps show the same rules (lib/password.ts, widgets/ui.dart).
pub fn password_problems(password: &str) -> Vec<&'static str> {
    let len = password.chars().count();
    let mut out = Vec::new();
    if len < MIN_PASSWORD_LEN {
        out.push("length");
    }
    let classes = [
        password.chars().any(|c| c.is_lowercase()),
        password.chars().any(|c| c.is_uppercase()),
        password.chars().any(|c| c.is_ascii_digit()),
        password.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if classes < 3 && len < PASSPHRASE_LEN {
        out.push("mix");
    }
    let letters: String = password.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
    let first = password.chars().next();
    let same_char = first.is_some() && password.chars().all(|c| Some(c) == first);
    let digits_only_or_sequence = letters.is_empty() || "abcdefghijklmnopqrstuvwxyz".contains(letters.as_str()) && letters.len() < 8;
    if same_char || digits_only_or_sequence || COMMON_PASSWORDS.contains(&letters.as_str()) {
        out.push("common");
    }
    out
}

fn is_local_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80,
        None => false,
    }
}

/// Normalizes a phone number to E.164 (best effort, no country inference except
/// for numbers starting with "00" or "+"). `default_prefix` like "+40" is used for
/// national numbers starting with a single 0.
pub fn normalize_phone(input: &str, default_prefix: Option<&str>) -> Option<String> {
    let trimmed = input.trim();
    let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
    let e164 = if trimmed.starts_with('+') {
        format!("+{digits}")
    } else if let Some(rest) = digits.strip_prefix("00") {
        format!("+{rest}")
    } else if let (Some(rest), Some(prefix)) = (digits.strip_prefix('0'), default_prefix) {
        format!("{prefix}{rest}")
    } else {
        return None;
    };
    let len = e164.len() - 1;
    ((7..=15).contains(&len) && !e164.starts_with("+0")).then_some(e164)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_policy() {
        for weak in ["short1A!", "alllowercaseletters", "Password123!", "Parola2024!!", "aaaaaaaaaaaaaaaa", "123456789012!", "Abcdef12345!"] {
            assert!(!password_problems(weak).is_empty(), "{weak} accepted");
        }
        for ok in ["Mere-Pere-2026", "corect horse battery staple", "Ploaie de vara 77", "x9!kQ2#vLm4p"] {
            assert!(password_problems(ok).is_empty(), "{ok} refused: {:?}", password_problems(ok));
        }
    }

    #[test]
    fn plain_http_only_for_local_servers() {
        assert_eq!(normalize_server_url("chat.example.com").as_deref(), Some("https://chat.example.com"));
        assert!(normalize_server_url("http://chat.example.com").is_none());
        assert!(normalize_server_url("http://8.8.8.8:8080").is_none());
        for ok in ["http://127.0.0.1:8080", "http://localhost:8080", "http://192.168.1.20:8080", "http://10.0.2.2:8080", "http://[::1]:8080"] {
            assert!(normalize_server_url(ok).is_some(), "{ok}");
        }
    }

    #[test]
    fn numbers() {
        assert_eq!(normalize_number("4821 0937-5512 8830").as_deref(), Some("4821093755128830"));
        assert_eq!(normalize_number("48210937551288"), None);
        assert_eq!(format_number("4821093755128830"), "4821 0937 5512 8830");
    }

    #[test]
    fn phones() {
        assert_eq!(normalize_phone("0722 123 456", Some("+40")).as_deref(), Some("+40722123456"));
        assert_eq!(normalize_phone("+39 347 1234567", None).as_deref(), Some("+393471234567"));
        assert_eq!(normalize_phone("0040722123456", None).as_deref(), Some("+40722123456"));
        assert_eq!(normalize_phone("12", None), None);
    }

    #[test]
    fn urls() {
        assert_eq!(normalize_server_url("chat.example.com/").as_deref(), Some("https://chat.example.com"));
        assert_eq!(normalize_server_url("http://localhost:8080").as_deref(), Some("http://localhost:8080"));
        assert_eq!(normalize_server_url("  "), None);
    }
}
