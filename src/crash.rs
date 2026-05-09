use std::fs;
use std::io::Write;
use std::panic;

pub fn install_panic_hook() {
    let default_hook = panic::take_hook();

    panic::set_hook(Box::new(move |info| {
        // Restore terminal FIRST so the user can see output
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stderr(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
        );

        let crash_report = build_crash_report(info);

        // Write to log file
        let log_path = write_crash_log(&crash_report);

        // Print to stderr so the user sees it
        eprintln!("\n\x1b[1;31mPhoenix crashed.\x1b[0m\n");
        eprintln!("{crash_report}");
        if let Some(path) = log_path {
            eprintln!("\x1b[2mCrash log saved to: {}\x1b[0m\n", path.display());
        }

        default_hook(info);
    }));
}

fn build_crash_report(info: &panic::PanicHookInfo<'_>) -> String {
    let mut report = String::new();

    // Location
    if let Some(loc) = info.location() {
        report.push_str(&format!(
            "at {}:{}:{}\n",
            loc.file(),
            loc.line(),
            loc.column()
        ));
    }

    // Message
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        report.push_str(&format!("message: {s}\n"));
    } else if let Some(s) = payload.downcast_ref::<String>() {
        report.push_str(&format!("message: {s}\n"));
    }

    // Backtrace
    let bt = std::backtrace::Backtrace::force_capture();
    report.push_str(&format!("\nbacktrace:\n{bt}"));

    report
}

fn write_crash_log(report: &str) -> Option<std::path::PathBuf> {
    let logs_dir = crate::config::paths::logs_dir();
    fs::create_dir_all(&logs_dir).ok()?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("crash_{timestamp}.log");
    let path = logs_dir.join(&filename);

    let mut file = fs::File::create(&path).ok()?;
    writeln!(file, "phoenix crash report").ok()?;
    writeln!(file, "time: {}", chrono::Utc::now().to_rfc3339()).ok()?;
    writeln!(file, "version: {}", env!("CARGO_PKG_VERSION")).ok()?;
    writeln!(file).ok()?;
    write!(file, "{report}").ok()?;

    // Prune old crash logs (keep last 20)
    if let Ok(mut entries) = fs::read_dir(&logs_dir) {
        let mut logs: Vec<_> = entries
            .by_ref()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("crash_") && n.ends_with(".log"))
            })
            .collect();
        if logs.len() > 20 {
            logs.sort_by_key(|e| e.file_name());
            for old in &logs[..logs.len() - 20] {
                let _ = fs::remove_file(old.path());
            }
        }
    }

    Some(path)
}
