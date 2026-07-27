#[derive(Clone)]
pub struct AppInfo {
    pub file: String,
    pub app_type: String,
    pub size: u64,
    pub is_running: bool,
    pub is_dir: bool,
}
