/// Format an ISO 8601 timestamp to "Mon DD, YYYY - HH:MM TZ" in local timezone.
/// Falls back to UTC display if local timezone conversion fails.
pub fn format_api_timestamp(ts: &str) -> String {
    use crate::usage::parse_iso8601_to_local;

    if let Some(zoned) = parse_iso8601_to_local(ts) {
        return zoned.strftime("%b %-d, %Y - %H:%M %Z").to_string();
    }

    // Fallback: parse and display as UTC
    format_api_timestamp_utc(ts)
}

/// UTC fallback for format_api_timestamp (when system timezone is unavailable).
fn format_api_timestamp_utc(ts: &str) -> String {
    let Ok(timestamp) = ts.parse::<jiff::Timestamp>() else {
        return ts.to_string();
    };
    let zoned = timestamp.to_zoned(jiff::tz::TimeZone::UTC);
    zoned.strftime("%b %-d, %Y - %H:%M UTC").to_string()
}

/// Capitalize the first letter of a string.
pub fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

/// Open a URL in the default browser. Fire-and-forget.
pub fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", url])
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_api_timestamp_valid() {
        let result = format_api_timestamp("2026-03-27T06:59:00.000Z");
        // Should contain date, time, and a timezone label
        assert!(result.contains("Mar"), "Expected month name, got: {}", result);
        assert!(result.contains(':'), "Expected HH:MM, got: {}", result);
        assert!(result.contains(','), "Expected 'Mon D, YYYY' format, got: {}", result);
        assert!(result.contains('-'), "Expected date-time separator, got: {}", result);
    }

    #[test]
    fn test_format_api_timestamp_utc_fallback() {
        let result = format_api_timestamp_utc("2026-03-27T11:21:00.000Z");
        assert_eq!(result, "Mar 27, 2026 - 11:21 UTC");
    }

    #[test]
    fn test_format_api_timestamp_invalid() {
        assert_eq!(format_api_timestamp("garbage"), "garbage");
        assert_eq!(format_api_timestamp("no-T-separator"), "no-T-separator");
    }
}
