use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use crate::config::PackageQueryConfig;

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

fn get_command_output(
    cmd: &str,
    args: &[impl AsRef<OsStr>],
    config: &PackageQueryConfig,
) -> Option<String> {
    let output = crate::command::output(
        Command::new(cmd).args(args),
        config.timeout_ms,
        config.max_output_bytes,
    )
    .ok()?;
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

fn query_pacman(exe_path: &Path, config: &PackageQueryConfig) -> Option<Vec<PathBuf>> {
    let pkg = get_command_output(
        "pacman",
        &[OsStr::new("-Qoq"), exe_path.as_os_str()],
        config,
    )?;
    let files = get_command_output("pacman", &["-Qlq", &pkg], config)?;
    Some(files.lines().map(PathBuf::from).collect())
}

fn query_dpkg(exe_path: &Path, config: &PackageQueryConfig) -> Option<Vec<PathBuf>> {
    let output = get_command_output(
        "dpkg-query",
        &[OsStr::new("-S"), exe_path.as_os_str()],
        config,
    )?;
    // Output format: "pkgname: /path/to/file" or "pkgname:arch: /path/to/file"
    let pkg = output.split(':').next()?.trim();
    let files = get_command_output("dpkg-query", &["-L", pkg], config)?;
    Some(files.lines().map(PathBuf::from).collect())
}

fn query_rpm(exe_path: &Path, config: &PackageQueryConfig) -> Option<Vec<PathBuf>> {
    // This covers Fedora (dnf), RHEL, openSUSE (zypper)
    let pkg = get_command_output(
        "rpm",
        &[
            OsStr::new("-qf"),
            exe_path.as_os_str(),
            OsStr::new("--queryformat"),
            OsStr::new("%{NAME}"),
        ],
        config,
    )?;
    let files = get_command_output("rpm", &["-ql", &pkg], config)?;
    Some(files.lines().map(PathBuf::from).collect())
}

fn query_portage(exe_path: &Path, config: &PackageQueryConfig) -> Option<Vec<PathBuf>> {
    let pkg = get_command_output("qfile", &[OsStr::new("-qC"), exe_path.as_os_str()], config)?;
    let files = get_command_output("qlist", &[&pkg], config)?;
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

fn query_traditional_package_manager(
    exe_path: &Path,
    config: &PackageQueryConfig,
) -> Option<Vec<PathBuf>> {
    if !is_system_package_path(exe_path) {
        return None;
    }
    match *TRADITIONAL_PACKAGE_MANAGER {
        Some(TraditionalPackageManager::Pacman) => query_pacman(exe_path, config),
        Some(TraditionalPackageManager::Dpkg) => query_dpkg(exe_path, config),
        Some(TraditionalPackageManager::Rpm) => query_rpm(exe_path, config),
        Some(TraditionalPackageManager::Portage) => query_portage(exe_path, config),
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

pub fn find_icon_via_package_manager(
    exe_path: &Path,
    config: &PackageQueryConfig,
) -> Option<PathBuf> {
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
        query_traditional_package_manager(exe_path, config)
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

    use super::{PackageQueryConfig, get_command_output, is_system_package_path, select_icon};

    #[test]
    fn traditional_queries_preserve_native_path_arguments() {
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
        use std::os::unix::fs::PermissionsExt as _;
        use std::path::PathBuf;
        use std::process::Command;

        const CHILD_PATH: &str = "CEFDETECTOR_TEST_PACKAGE_PATH";
        const ICON_PATH: &str = "CEFDETECTOR_TEST_PACKAGE_ICON";
        const TRACE_PATH: &str = "CEFDETECTOR_TEST_PACKAGE_TRACE";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let icon = PathBuf::from(std::env::var_os(ICON_PATH).unwrap());
            let config = PackageQueryConfig {
                timeout_ms: 2000,
                max_output_bytes: 65536,
            };
            assert_eq!(
                super::find_icon_via_package_manager(Path::new(&path), &config),
                Some(icon)
            );
            return;
        }

        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let root = Fixture(
            std::env::temp_dir().join(format!("cefdetector-package-native-{}", std::process::id())),
        );
        fs::create_dir(&root.0).unwrap();
        let icon = root.0.join("share/icons/256x256/apps/native.png");
        fs::create_dir_all(icon.parent().unwrap()).unwrap();
        fs::write(&icon, include_bytes!("../icons/32x32.png")).unwrap();
        let script = r#"#!/bin/sh
case "$0" in
  */pacman) program=pacman ;;
  */dpkg-query) program=dpkg-query ;;
  */rpm) program=rpm ;;
  */qfile) program=qfile ;;
  */qlist) program=qlist ;;
  *) exit 10 ;;
esac
printf '%s\000' "$program" "$@" >> "$CEFDETECTOR_TEST_PACKAGE_TRACE"
case "$program:$1" in
  pacman:-Qoq|dpkg-query:-S|rpm:-qf|qfile:-qC)
    [ "$2" = "$CEFDETECTOR_TEST_PACKAGE_PATH" ] || exit 9
    if [ "$program" = dpkg-query ]; then
      printf '  native-package: ignored-path  \n'
    else
      printf '  native-package  \n'
    fi
    ;;
  pacman:-Qlq|dpkg-query:-L|rpm:-ql|qlist:native-package)
    printf '%s\n' "$CEFDETECTOR_TEST_PACKAGE_ICON"
    ;;
  *) exit 10 ;;
esac
"#;
        let mut failures = Vec::new();
        for manager in ["pacman", "dpkg-query", "rpm", "portage"] {
            let bin = root.0.join(manager);
            fs::create_dir(&bin).unwrap();
            let commands = if manager == "portage" {
                vec!["qfile", "qlist"]
            } else {
                vec![manager]
            };
            for command in commands {
                let path = bin.join(command);
                fs::write(&path, script).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            for (index, name) in [
                OsString::from_vec(b"app-\xff".to_vec()),
                OsString::from_vec(b"app-\xfe".to_vec()),
                OsString::from("中文 app '\"$\n"),
            ]
            .into_iter()
            .enumerate()
            {
                // This path is only an argument to the mocks; nothing is written under /opt.
                let path = Path::new("/opt/CefDetector native test").join(name);
                let trace = root.0.join(format!("{manager}-{index}.trace"));
                let output = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "package_manager::tests::traditional_queries_preserve_native_path_arguments",
                    ])
                    .env("PATH", &bin)
                    .env(CHILD_PATH, &path)
                    .env(ICON_PATH, &icon)
                    .env(TRACE_PATH, &trace)
                    .output()
                    .unwrap();
                if !output.status.success() {
                    failures.push(format!(
                        "{manager}/{index}:\n{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ));
                    continue;
                }
                let path = path.as_os_str().as_bytes();
                let expected: Vec<&[u8]> = match manager {
                    "pacman" => vec![
                        b"pacman",
                        b"-Qoq",
                        path,
                        b"pacman",
                        b"-Qlq",
                        b"native-package",
                    ],
                    "dpkg-query" => vec![
                        b"dpkg-query",
                        b"-S",
                        path,
                        b"dpkg-query",
                        b"-L",
                        b"native-package",
                    ],
                    "rpm" => vec![
                        b"rpm",
                        b"-qf",
                        path,
                        b"--queryformat",
                        b"%{NAME}",
                        b"rpm",
                        b"-ql",
                        b"native-package",
                    ],
                    "portage" => vec![b"qfile", b"-qC", path, b"qlist", b"native-package"],
                    _ => unreachable!(),
                };
                let bytes = fs::read(trace).unwrap();
                let actual: Vec<_> = bytes
                    .strip_suffix(b"\0")
                    .unwrap()
                    .split(|byte| *byte == 0)
                    .collect();
                assert_eq!(actual, expected, "{manager}/{index}");
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn command_output_preserves_trimming_and_failure_semantics() {
        let config = PackageQueryConfig {
            timeout_ms: 2000,
            max_output_bytes: 64,
        };
        assert_eq!(
            get_command_output(
                "/bin/sh",
                &["-c", "printf '  package-name \\n'; printf ignored >&2"],
                &config,
            ),
            Some("package-name".into())
        );
        assert_eq!(
            get_command_output("/bin/sh", &["-c", "printf package-name; exit 1"], &config),
            None
        );
    }

    #[test]
    fn command_limits_apply_to_raw_streams_before_parsing() {
        let config = PackageQueryConfig {
            timeout_ms: 100,
            max_output_bytes: 3,
        };
        assert_eq!(
            get_command_output("/bin/sh", &["-c", "printf abc"], &config),
            Some("abc".into())
        );
        for script in ["printf 'x    '", "printf ok; printf 1234 >&2"] {
            assert_eq!(
                get_command_output("/bin/sh", &["-c", script], &config),
                None
            );
        }
        assert_eq!(get_command_output("/bin/sleep", &["0.6"], &config), None);
    }

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
