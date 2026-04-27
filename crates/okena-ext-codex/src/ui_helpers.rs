/// Format an ISO 8601 timestamp to "Mon DD, YYYY - HH:MM UTC"
pub fn format_api_timestamp(ts: &str) -> String {
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
    okena_core::process::open_url(url);
}
