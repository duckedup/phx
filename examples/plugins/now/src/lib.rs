use phoenix_plugin_sdk::tool;

tool! {
    tools: [
        {
            name: "get_current_time",
            description: "Get the current date and time in UTC. Returns an ISO 8601 timestamp.",
            parameters: r#"{"type":"object","properties":{}}"#,
            invoke(_name, _args) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?;

                let total_secs = now.as_secs();
                let days_since_epoch = total_secs / 86400;
                let time_of_day = total_secs % 86400;
                let hours = time_of_day / 3600;
                let minutes = (time_of_day % 3600) / 60;
                let seconds = time_of_day % 60;

                let mut days = days_since_epoch as i64;
                let mut year = 1970i32;
                loop {
                    let days_in_year = if is_leap(year) { 366 } else { 365 };
                    if days < days_in_year {
                        break;
                    }
                    days -= days_in_year;
                    year += 1;
                }
                let month_days: &[i64] = if is_leap(year) {
                    &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                } else {
                    &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                };
                let mut month = 0usize;
                for &md in month_days {
                    if days < md {
                        break;
                    }
                    days -= md;
                    month += 1;
                }
                let day = days + 1;

                let timestamp = format!(
                    "{year}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                    month + 1, day, hours, minutes, seconds
                );

                Ok((format!("The current date and time is {timestamp} (UTC)."), false))
            }
        }
    ]
}

fn is_leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
