use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};

static PM_CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Copy)]
enum TraditionalPackageManager {
    Pacman,
    Dpkg,
    Rpm,
    Portage,
}

static TRADITIONAL_PACKAGE_MANAGER: LazyLock<Option<TraditionalPackageManager>> =
    LazyLock::new(|| {
        if command_exists("pacman") {
            Some(TraditionalPackageManager::Pacman)
        } else if command_exists("dpkg-query") {
            Some(TraditionalPackageManager::Dpkg)
        } else if command_exists("rpm") {
            Some(TraditionalPackageManager::Rpm)
        } else if command_exists("qfile") && command_exists("qlist") {
            Some(TraditionalPackageManager::Portage)
        } else {
            None
        }
    });

fn command_exists(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| fs_metadata_is_executable(&dir.join(command)))
}

fn fs_metadata_is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn get_command_output(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn collect_files_recursive(dirs: &[PathBuf], files: &mut Vec<PathBuf>) {
    for dir in dirs {
        let mut stack = vec![dir.clone()];
        while let Some(current) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&current) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    if file_type.is_dir() {
                        stack.push(path);
                    } else if file_type.is_file() {
                        files.push(path);
                    }
                }
            }
        }
    }
}

// Traditional package managers (pacman, dpkg, rpm, portage)

fn query_pacman(exe_path: &str) -> Option<Vec<PathBuf>> {
    let pkg = get_command_output("pacman", &["-Qoq", exe_path])?;
    let files = get_command_output("pacman", &["-Qlq", &pkg])?;
    Some(files.lines().map(PathBuf::from).collect())
}

fn query_dpkg(exe_path: &str) -> Option<Vec<PathBuf>> {
    let output = get_command_output("dpkg-query", &["-S", exe_path])?;
    // Output format: "pkgname: /path/to/file" or "pkgname:arch: /path/to/file"
    let pkg = output.split(':').next()?.trim();
    let files = get_command_output("dpkg-query", &["-L", pkg])?;
    Some(files.lines().map(PathBuf::from).collect())
}

fn query_rpm(exe_path: &str) -> Option<Vec<PathBuf>> {
    // This covers Fedora (dnf), RHEL, openSUSE (zypper)
    let pkg = get_command_output("rpm", &["-qf", exe_path, "--queryformat", "%{NAME}"])?;
    let files = get_command_output("rpm", &["-ql", &pkg])?;
    Some(files.lines().map(PathBuf::from).collect())
}

fn query_portage(exe_path: &str) -> Option<Vec<PathBuf>> {
    let pkg = get_command_output("qfile", &["-qC", exe_path])?;
    let files = get_command_output("qlist", &[&pkg])?;
    Some(files.lines().map(PathBuf::from).collect())
}

// Path-based package managers (snap, flatpak, nix, brew)

fn query_snap(exe_path: &Path) -> Option<Vec<PathBuf>> {
    // Snap paths are typically /snap/<name>/<revision>/...
    let mut components = exe_path.components();
    if components.next()?.as_os_str() != "/" {
        return None;
    }
    if components.next()?.as_os_str() != "snap" {
        return None;
    }
    let snap_name = components.next()?.as_os_str();
    let revision = components.next()?.as_os_str();

    let meta_gui = Path::new("/snap")
        .join(snap_name)
        .join(revision)
        .join("meta/gui");

    // Snaps usually store icons in meta/gui, let's just collect all files there
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(meta_gui) {
        for entry in entries.flatten() {
            files.push(entry.path());
        }
    }

    // Also add system snap exports
    files.push(
        Path::new("/var/lib/snapd/desktop/applications").join(format!(
            "{}_{}.desktop",
            snap_name.to_string_lossy(),
            snap_name.to_string_lossy()
        )),
    );

    Some(files)
}

fn query_flatpak(exe_path: &Path) -> Option<Vec<PathBuf>> {
    // Flatpak app paths: /var/lib/flatpak/app/<app-id>/<arch>/<branch>/<hash>/...
    if !exe_path.starts_with("/var/lib/flatpak/app") {
        return None;
    }

    let path_str = exe_path.to_string_lossy();
    let parts: Vec<&str> = path_str.split('/').collect();
    if parts.len() < 7 {
        return None;
    }
    let app_id = parts[5];

    // Flatpak exports icons to /var/lib/flatpak/exports/share/icons/...
    let mut files = Vec::new();

    let export_dir = Path::new("/var/lib/flatpak/exports/share/icons/hicolor");
    if let Ok(entries) = std::fs::read_dir(export_dir) {
        for size_dir in entries.flatten() {
            let app_dir = size_dir.path().join("apps");
            let icon_png = app_dir.join(format!("{}.png", app_id));
            let icon_svg = app_dir.join(format!("{}.svg", app_id));
            if icon_png.exists() {
                files.push(icon_png);
            }
            if icon_svg.exists() {
                files.push(icon_svg);
            }
        }
    }

    Some(files)
}

fn query_nix(exe_path: &Path) -> Option<Vec<PathBuf>> {
    // Nix paths: /nix/store/<hash>-<name>-<version>/bin/...
    if !exe_path.starts_with("/nix/store") {
        return None;
    }

    let path_str = exe_path.to_string_lossy();
    let parts: Vec<&str> = path_str.split('/').collect();
    if parts.len() < 4 {
        return None;
    }
    let store_root = format!("/nix/store/{}", parts[3]);

    let mut files = Vec::new();
    collect_files_recursive(
        &[
            Path::new(&store_root).join("share/icons"),
            Path::new(&store_root).join("share/pixmaps"),
        ],
        &mut files,
    );

    Some(files)
}

fn query_brew(exe_path: &Path) -> Option<Vec<PathBuf>> {
    // Brew paths: /home/linuxbrew/.linuxbrew/Cellar/<name>/<version>/bin/...
    let path_str = exe_path.to_string_lossy();
    if !path_str.contains(".linuxbrew/Cellar/") {
        return None;
    }

    let parts: Vec<&str> = path_str.split('/').collect();
    // find index of "Cellar"
    let cellar_idx = parts.iter().position(|&r| r == "Cellar")?;
    if parts.len() < cellar_idx + 3 {
        return None;
    }

    let root_path = parts[..cellar_idx + 3].join("/");

    let mut files = Vec::new();
    collect_files_recursive(
        &[
            Path::new(&root_path).join("share/icons"),
            Path::new(&root_path).join("share/pixmaps"),
        ],
        &mut files,
    );

    Some(files)
}

pub fn clear_pm_cache() {
    PM_CACHE.lock().unwrap().clear();
}

fn is_system_package_path(exe_path: &Path) -> bool {
    ["/usr", "/opt", "/bin", "/sbin", "/lib", "/lib64"]
        .iter()
        .any(|root| exe_path.starts_with(root))
}

fn query_traditional_package_manager(exe_path: &Path) -> Option<Vec<PathBuf>> {
    if !is_system_package_path(exe_path) {
        return None;
    }
    let path = exe_path.to_string_lossy();
    match *TRADITIONAL_PACKAGE_MANAGER {
        Some(TraditionalPackageManager::Pacman) => query_pacman(&path),
        Some(TraditionalPackageManager::Dpkg) => query_dpkg(&path),
        Some(TraditionalPackageManager::Rpm) => query_rpm(&path),
        Some(TraditionalPackageManager::Portage) => query_portage(&path),
        None => None,
    }
}

fn select_icon(files: Vec<PathBuf>) -> Option<PathBuf> {
    // Find .desktop file to see if it specifies an icon name
    let mut icon_name = None;
    for file in &files {
        if file
            .extension()
            .is_some_and(|extension| extension == "desktop")
            && let Ok(content) = std::fs::read_to_string(file)
        {
            for line in content.lines() {
                if let Some(icon) = line.strip_prefix("Icon=") {
                    icon_name = Some(icon.trim().to_owned());
                    break;
                }
            }
        }
    }

    // Try to find the exact icon specified in .desktop
    if let Some(name) = icon_name {
        for file in &files {
            if !file.is_file() {
                continue;
            }
            let file_name = file.file_name().unwrap_or_default().to_string_lossy();
            let is_image = file_name.ends_with(".png")
                || file_name.ends_with(".svg")
                || file_name.ends_with(".xpm");
            if is_image {
                let file_stem = file.file_stem().unwrap_or_default().to_string_lossy();
                if file_stem == name || file_name == name {
                    return Some(file.clone());
                }
            }
        }
    }

    // Fallback: look for any likely icon file
    let mut best_icon: Option<PathBuf> = None;
    let mut max_size = 0;

    for file in files {
        if !file.is_file() {
            continue;
        }
        let path_str = file.to_string_lossy().to_lowercase();
        if (path_str.ends_with(".png") || path_str.ends_with(".svg") || path_str.ends_with(".xpm"))
            && (path_str.contains("icons/")
                || path_str.contains("pixmaps/")
                || path_str.contains("meta/gui/"))
        {
            // If it's a PNG, prefer larger resolutions
            if path_str.ends_with(".png") {
                let mut size = 0;
                if path_str.contains("256x256") {
                    size = 256;
                } else if path_str.contains("128x128") {
                    size = 128;
                } else if path_str.contains("64x64") {
                    size = 64;
                } else if path_str.contains("48x48") {
                    size = 48;
                } else if path_str.contains("32x32") {
                    size = 32;
                }

                if size > max_size {
                    max_size = size;
                    best_icon = Some(file);
                }
            } else if best_icon.is_none() {
                best_icon = Some(file);
            }
        }
    }

    best_icon
}

pub fn find_icon_via_package_manager(exe_path: &Path) -> Option<PathBuf> {
    if let Some(cached) = PM_CACHE.lock().unwrap().get(exe_path) {
        return cached.clone();
    }

    let files = if exe_path.starts_with("/nix/store") {
        query_nix(exe_path)
    } else if exe_path.starts_with("/snap") {
        query_snap(exe_path)
    } else if exe_path.starts_with("/var/lib/flatpak/app") {
        query_flatpak(exe_path)
    } else if exe_path.to_string_lossy().contains(".linuxbrew/Cellar/") {
        query_brew(exe_path)
    } else {
        query_traditional_package_manager(exe_path)
    };

    let best_icon = files.and_then(select_icon);

    PM_CACHE
        .lock()
        .unwrap()
        .insert(exe_path.to_path_buf(), best_icon.clone());
    best_icon
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_system_package_path, select_icon};

    #[test]
    fn traditional_package_queries_are_limited_to_system_paths() {
        assert!(is_system_package_path(Path::new("/usr/bin/example")));
        assert!(is_system_package_path(Path::new("/opt/example/app")));
        assert!(!is_system_package_path(Path::new(
            "/home/user/.local/bin/example"
        )));
    }

    #[test]
    fn empty_package_file_list_has_no_icon() {
        assert_eq!(select_icon(Vec::new()), None);
    }
}
