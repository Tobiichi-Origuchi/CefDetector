use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::models::AppInfo;

fn match_file_patterns(name: &str) -> (bool, bool, bool) {
    let matched_pak = name.contains("_100_") && name.ends_with(".pak");
    let matched_cef = name.contains("libcef");
    let matched_node = name
        .find("libnode")
        .is_some_and(|start| name[start + "libnode".len()..].contains(".so"));
    (matched_pak, matched_cef, matched_node)
}

pub fn open_path(path: String, is_dir: bool) {
    if path.contains("://") || is_dir {
        let _ = Command::new("xdg-open").arg(path).spawn();
    } else {
        if let Some(p) = Path::new(&path).parent() {
            let _ = Command::new("xdg-open").arg(p).spawn();
        }
    }
}

use ignore::{WalkBuilder, WalkState};

struct IgnoreConfig {
    dir_names: HashSet<String>,
    abs_paths: HashSet<PathBuf>,
}

fn load_ignore_config() -> IgnoreConfig {
    let mut config = IgnoreConfig {
        dir_names: HashSet::new(),
        abs_paths: HashSet::new(),
    };

    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });

    let ignore_file = config_dir.join("cefdetector").join(".ignore");
    if let Ok(content) = std::fs::read_to_string(ignore_file) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('/') {
                config.abs_paths.insert(PathBuf::from(line));
            } else {
                config.dir_names.insert(line.to_string());
            }
        }
    }

    config
}

struct ScanResults {
    pak_files: Vec<String>,
    cef_files: Vec<String>,
    node_files: Vec<String>,
}

fn single_pass_scan() -> ScanResults {
    let pak_results = Arc::new(Mutex::new(Vec::new()));
    let cef_results = Arc::new(Mutex::new(Vec::new()));
    let node_results = Arc::new(Mutex::new(Vec::new()));

    let ignore_config = load_ignore_config();

    let mut builder = WalkBuilder::new("/");
    builder
        .standard_filters(false)
        .hidden(false)
        .threads(
            std::thread::available_parallelism()
                .map(|n| n.get().min(8))
                .unwrap_or(4),
        )
        .filter_entry(move |entry| {
            let path = entry.path();
            if path.starts_with("/proc")
                || path.starts_with("/sys")
                || path.starts_with("/dev")
                || path.starts_with("/run")
                || path.starts_with("/tmp")
                || path.starts_with("/boot")
                || path.starts_with("/lost+found")
            {
                return false;
            }

            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if ignore_config.abs_paths.contains(path) {
                    return false;
                }
                if let Some(name) = path.file_name()
                    && ignore_config
                        .dir_names
                        .contains(name.to_string_lossy().as_ref())
                {
                    return false;
                }
            }

            true
        });

    builder.build_parallel().run(|| {
        let pak = Arc::clone(&pak_results);
        let cef = Arc::clone(&cef_results);
        let node = Arc::clone(&node_results);

        Box::new(move |result| {
            if let Ok(entry) = result
                && let Some(file_type) = entry.file_type()
                && file_type.is_file()
            {
                let name = entry.file_name().to_string_lossy();
                let (matched_pak, matched_cef, matched_node) = match_file_patterns(&name);

                if matched_pak || matched_cef || matched_node {
                    let path_str = entry.path().to_string_lossy().into_owned();
                    if matched_pak {
                        pak.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(path_str.clone());
                    }
                    if matched_cef {
                        cef.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(path_str.clone());
                    }
                    if matched_node {
                        node.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(path_str);
                    }
                }
            }
            WalkState::Continue
        })
    });

    ScanResults {
        pak_files: std::mem::take(&mut *pak_results.lock().unwrap()),
        cef_files: std::mem::take(&mut *cef_results.lock().unwrap()),
        node_files: std::mem::take(&mut *node_results.lock().unwrap()),
    }
}

fn dir_size(dir: &Path, cache: &mut HashSet<u64>, deep: u32) -> u64 {
    if deep > 10 {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                let ino = meta.ino();
                if cache.insert(ino) {
                    total += meta.size();
                    if meta.is_dir() {
                        total += dir_size(&entry.path(), cache, deep + 1);
                    }
                }
            }
        }
    }
    total
}

fn get_running_processes() -> HashSet<String> {
    let mut procs = HashSet::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();
            if file_name_str.chars().all(char::is_numeric) {
                let exe_path = entry.path().join("exe");
                if let Ok(link) = fs::read_link(&exe_path) {
                    procs.insert(link.to_string_lossy().into_owned());
                }
            }
        }
    }
    procs
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(haystack, needle).is_some()
}

pub fn core_search<F>(mut on_found: F)
where
    F: FnMut(AppInfo),
{
    let running_procs = get_running_processes();
    let mut found_files = HashSet::new();
    let mut visited_dirs = HashSet::new();
    let mut global_ino_cache = HashSet::new();

    let mut add_app = |file: &Path, app_type: &str, is_dir: bool| {
        let file_str = file.to_string_lossy().into_owned();
        if !found_files.insert(file_str.clone()) {
            return;
        }
        let target_dir = if is_dir {
            file.to_path_buf()
        } else {
            file.parent().unwrap().to_path_buf()
        };
        let size = dir_size(&target_dir, &mut global_ino_cache, 0);

        let is_running = if !is_dir {
            running_procs.contains(&file_str)
        } else {
            false
        };

        on_found(AppInfo {
            file: file_str,
            app_type: app_type.to_owned(),
            size,
            is_running,
            is_dir,
        });
    };

    let search_dir = |dir: &Path| -> (bool, Option<PathBuf>, Option<String>) {
        let msedge = dir.join("msedge");
        if msedge.exists() {
            return (true, Some(msedge), Some("Edge".to_string()));
        }
        // Check Linux Chrome equivalents
        let chrome = dir.join("chrome");
        if chrome.exists() {
            return (true, Some(chrome), Some("Chrome".to_string()));
        }

        let mut first_exe = None;
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if !meta.is_file() {
                    continue;
                }

                // Executables and .so files may embed CEF signatures
                let is_so = path.extension().is_some_and(|e| e == "so");
                let is_exec = meta.permissions().mode() & 0o111 != 0;

                if !is_so && !is_exec {
                    continue;
                }

                let file = match fs::File::open(&path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                // SAFETY: read-only mmap; SIGBUS possible if file is truncated concurrently
                let mmap = unsafe { memmap2::MmapOptions::new().map(&file) };
                if let Ok(data) = mmap {
                    let typ = if contains_bytes(&data, b"third_party/electron_node")
                        || contains_bytes(&data, b"register_atom_browser_web_contents")
                    {
                        "Electron"
                    } else if contains_bytes(&data, b"url-nwjs") {
                        "NWJS"
                    } else if contains_bytes(&data, b"CefSharp.Internals") {
                        "CefSharp"
                    } else if contains_bytes(&data, b"cef_string_utf8_to_utf16") {
                        "CEF"
                    } else {
                        if first_exe.is_none() {
                            let fname = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_lowercase();
                            if !fname.contains("unins")
                                && !fname.contains("setup")
                                && !fname.contains("report")
                                && is_exec
                            {
                                first_exe = Some(path.clone());
                            }
                        }
                        continue;
                    };

                    return (true, Some(path), Some(typ.to_string()));
                }
            }
        }
        (false, first_exe, None)
    };

    let mut search_cef = |stdout: Vec<String>, default_type: &str| {
        for file in stdout {
            if file.contains("/.Trash")
                || file.contains("/Trash/")
                || file.to_lowercase().ends_with(".log")
            {
                continue;
            }
            let path = Path::new(&file);
            let dir = if let Some(p) = path.parent() {
                p
            } else {
                continue;
            };

            if !visited_dirs.insert(dir.to_string_lossy().into_owned()) {
                continue;
            }

            if path.is_dir() {
                continue;
            }

            let (found, exe, typ) = search_dir(dir);
            if found {
                add_app(&exe.unwrap(), &typ.unwrap(), false);
                continue;
            }
            if let Some(e) = exe {
                add_app(&e, default_type, false);
            } else {
                let parent_dir = if let Some(p) = dir.parent() {
                    p
                } else {
                    continue;
                };
                let (found2, exe2, typ2) = search_dir(parent_dir);
                if found2 {
                    add_app(&exe2.unwrap(), &typ2.unwrap(), false);
                    continue;
                }
                if let Some(e) = exe2 {
                    add_app(&e, default_type, false);
                } else {
                    add_app(dir, default_type, true);
                }
            }
        }
    };

    let scan = single_pass_scan();

    search_cef(scan.pak_files, "Unknown");
    search_cef(scan.cef_files, "CEF");

    let node_dlls = scan.node_files;
    for file in node_dlls {
        if file.contains("/.Trash") || file.contains("/Trash/") {
            continue;
        }
        let path = Path::new(&file);
        if path.is_dir() {
            continue;
        }
        let dir = if let Some(p) = path.parent() {
            p
        } else {
            continue;
        };

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if !meta.is_file() {
                    continue;
                }
                let is_exec = meta.permissions().mode() & 0o111 != 0;
                let is_so = path.extension().is_some_and(|e| e == "so");

                if (is_exec || is_so)
                    && let Ok(file_handle) = fs::File::open(&path)
                {
                    // SAFETY: read-only mmap; SIGBUS possible if file is truncated concurrently
                    let mmap = unsafe { memmap2::MmapOptions::new().map(&file_handle) };
                    if let Ok(data) = mmap {
                        let typ = if contains_bytes(&data, b"napi_create_buffer") {
                            "Mini Electron"
                        } else if contains_bytes(&data, b"miniblink") {
                            "Mini Blink"
                        } else {
                            continue;
                        };

                        add_app(&path, typ, false);
                        break;
                    }
                }
            }
        }
    }
}
