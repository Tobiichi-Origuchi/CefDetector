use std::path::PathBuf;

#[derive(Clone)]
pub struct AppInfo {
    pub file: PathBuf,
    pub app_type: String,
    pub size: u64,
    pub is_running: bool,
    pub is_dir: bool,
}
