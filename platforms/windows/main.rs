#![windows_subsystem = "windows"]

use nsh_platform_windows as platform;

fn main() -> impl std::process::Termination {
    env_logger::init();
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<nemorous::App>()),
        platform::AppOptions {
            title: "Nemorous".to_owned(),
            size: (1440, 900),
            ..Default::default()
        },
    )
}
