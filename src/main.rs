#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;

fn initial_window_size() -> (i32, i32) {
    if let Ok(raw) = std::env::var("NEMOROUS_WINDOW_SIZE") {
        if let Some((w, h)) = raw.trim().split_once('x') {
            if let (Ok(wi), Ok(hi)) = (w.parse::<i32>(), h.parse::<i32>()) {
                if wi >= 640 && hi >= 420 {
                    return (wi, hi);
                }
            }
        }
    }
    (1440, 900)
}

#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    env_logger::init();
    if std::env::var_os("NEMOROUS_TRACE_TIMING").is_some() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        eprintln!("NEMOROUS_TIMING event=main_start epoch_ms={now_ms}");
    }
    let _gesture_monitor = nemorous::macos_gestures::install();
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<nemorous::App>()),
        platform::AppOptions {
            title: "Nemorous".to_owned(),
            size: initial_window_size(),
            ..Default::default()
        },
    )
}
