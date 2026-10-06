const COMMANDS: &[&str] = &["get_power_state", "set_power_profile"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
