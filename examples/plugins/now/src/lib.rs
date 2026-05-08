use phoenix_plugin_sdk::skill;
use phoenix_plugin_sdk::ui::{BoxNode, TextNode};

skill! {
    name: "now",
    command: "now",
    description: "Inject the current date and time into the session context",
    execute(arguments) {
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

        let extra = if arguments.is_empty() {
            String::new()
        } else {
            format!(" {arguments}")
        };

        let month_names = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun",
            "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let month_name = month_names[month];
        let display_time = format!("{:02}:{:02}:{:02} UTC", hours, minutes, seconds);
        let display_date = format!("{day} {month_name} {year}");

        Ok(phoenix_plugin_sdk::SkillResult {
            context: format!("The current date and time is {timestamp} (UTC).{extra}"),
            toast: String::new(),
            widget: Some(
                BoxNode::new("Current Time")
                    .child(TextNode::new(&display_time).bold().fg("cyan"))
                    .child(TextNode::new(&display_date).dim())
                    .into_node()
            ),
        })
    }
}

fn is_leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
