/// Format an ISO 8601 timestamp to "Mon DD, YYYY - HH:MM TZ" in local timezone.
/// Falls back to UTC display if local timezone conversion fails.
pub fn format_api_timestamp(ts: &str) -> String {
    use crate::usage::{epoch_to_local_time, parse_iso8601_to_epoch};

    if let Some(epoch) = parse_iso8601_to_epoch(ts) {
        if let Some(local) = epoch_to_local_time(epoch) {
            let tz_label = if local.tz_abbr.is_empty() {
                "UTC".to_string()
            } else {
                local.tz_abbr
            };
            let month_name = match local.month {
                1 => "Jan",  2 => "Feb",  3 => "Mar",  4 => "Apr",
                5 => "May",  6 => "Jun",  7 => "Jul",  8 => "Aug",
                9 => "Sep", 10 => "Oct", 11 => "Nov", 12 => "Dec",
                _ => "?",
            };
            return format!(
                "{} {}, {} - {:02}:{:02} {}",
                month_name, local.day, local.year, local.hour, local.min, tz_label
            );
        }
    }

    // Fallback: parse and display as UTC
    format_api_timestamp_utc(ts)
}

/// UTC fallback for format_api_timestamp.
fn format_api_timestamp_utc(ts: &str) -> String {
    let parts: Vec<&str> = ts.split('T').collect();
    if parts.len() != 2 {
        return ts.to_string();
    }
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return ts.to_string();
    }
    let time = parts[1].split('.').next().unwrap_or(parts[1]);
    let time = time.trim_end_matches('Z');
    let hm: Vec<&str> = time.split(':').collect();
    if hm.len() < 2 {
        return ts.to_string();
    }

    let month_name = match date_parts[1] {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => date_parts[1],
    };

    format!(
        "{} {}, {} - {}:{} UTC",
        month_name, date_parts[2], date_parts[0], hm[0], hm[1]
    )
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
        // Should NOT end with "UTC" unless system is in UTC
        // (we can't assert exact tz, but format should be "Mon D, YYYY - HH:MM TZ")
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
