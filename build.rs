fn main() {
    if std::env::var_os("CARGO_FEATURE_SETTINGS_UI").is_some() {
        slint_build::compile("ui/settings.slint")
            .expect("failed to compile the Crosshair Settings UI");
    }
}
