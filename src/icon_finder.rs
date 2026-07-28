use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

#[derive(Clone)]
pub enum RawIcon {
    Svg(Arc<[u8]>),
    PngOrIco(Arc<[u8]>),
    Empty,
}

static DEFAULT_ICON: LazyLock<RawIcon> = LazyLock::new(|| {
    RawIcon::PngOrIco(Arc::from(
        include_bytes!("../icons/default_cef_icon.ico").as_slice(),
    ))
});

static RAW_ICON_CACHE: LazyLock<Mutex<HashMap<PathBuf, RawIcon>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static DESKTOP_CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static DESKTOP_INDEX: LazyLock<Mutex<Option<HashMap<String, String>>>> =
    LazyLock::new(|| Mutex::new(None));

static ICON_THEME_CACHE: LazyLock<Mutex<HashMap<String, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static NEIGHBOR_ICON_CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn try_icon_from_path(path: &Path) -> Option<RawIcon> {
    let bytes: Arc<[u8]> = fs::read(path).ok()?.into();
    let is_svg = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("svg"))
        .unwrap_or(false);
    Some(if is_svg {
        RawIcon::Svg(bytes)
    } else {
        RawIcon::PngOrIco(bytes)
    })
}

pub fn find_icon_in_theme(icon_name: &str) -> Option<PathBuf> {
    if let Some(cached) = ICON_THEME_CACHE.lock().unwrap().get(icon_name) {
        return cached.clone();
    }

    let search_dirs: &[&str] = &[
        "/usr/share/pixmaps",
        "/usr/share/icons/hicolor/512x512/apps",
        "/usr/share/icons/hicolor/256x256/apps",
        "/usr/share/icons/hicolor/128x128/apps",
        "/usr/share/icons/hicolor/64x64/apps",
        "/usr/share/icons/hicolor/48x48/apps",
        "/usr/share/icons/hicolor/32x32/apps",
        "/usr/share/icons/hicolor/scalable/apps",
        "/usr/share/icons/Adwaita/512x512/apps",
        "/usr/share/icons/Adwaita/256x256/apps",
        "/usr/share/icons/Adwaita/scalable/apps",
    ];

    let home_dirs: Option<[String; 5]> = std::env::var("HOME").ok().map(|h| {
        [
            format!("{}/.local/share/icons/hicolor/512x512/apps", h),
            format!("{}/.local/share/icons/hicolor/256x256/apps", h),
            format!("{}/.local/share/icons/hicolor/128x128/apps", h),
            format!("{}/.local/share/icons/hicolor/scalable/apps", h),
            format!("{}/.local/share/icons", h),
        ]
    });

    let result = {
        let mut found = None;
        for ext in ["png", "svg"] {
            for dir in search_dirs {
                let p = Path::new(dir).join(format!("{}.{}", icon_name, ext));
                if p.exists() {
                    found = Some(p);
                    break;
                }
            }
            if found.is_none()
                && let Some(ref dirs) = home_dirs
            {
                for dir in dirs {
                    let p = Path::new(dir).join(format!("{}.{}", icon_name, ext));
                    if p.exists() {
                        found = Some(p);
                        break;
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        found
    };

    ICON_THEME_CACHE
        .lock()
        .unwrap()
        .insert(icon_name.to_owned(), result.clone());
    result
}

pub fn find_neighboring_icon(exe_path: &Path) -> Option<PathBuf> {
    let parent = exe_path.parent()?;

    if let Some(cached) = NEIGHBOR_ICON_CACHE.lock().unwrap().get(parent) {
        return cached.clone();
    }

    let exe_name = exe_path.file_name()?.to_string_lossy().to_string();

    let result = (|| {
        let mut dirs_to_check = vec![parent.to_path_buf()];
        let resources_dir = parent.join("resources");
        if resources_dir.exists() {
            dirs_to_check.push(resources_dir);
        }
        let assets_dir = parent.join("assets");
        if assets_dir.exists() {
            dirs_to_check.push(assets_dir);
        }

        let name_pat = [format!("{}.png", exe_name), format!("{}.svg", exe_name)];
        let name_fixed = ["icon.png", "logo.png", "app.png"];

        for dir in dirs_to_check {
            for name in name_pat
                .iter()
                .map(|s| s.as_str())
                .chain(name_fixed.iter().copied())
            {
                let p = dir.join(name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        None
    })();

    NEIGHBOR_ICON_CACHE
        .lock()
        .unwrap()
        .insert(parent.to_path_buf(), result.clone());
    result
}

pub fn find_icon_via_desktop_file(exe_path: &Path) -> Option<PathBuf> {
    if let Some(cached) = DESKTOP_CACHE.lock().unwrap().get(exe_path) {
        return cached.clone();
    }

    let exe_name = exe_path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let icon = {
        let mut index = DESKTOP_INDEX.lock().unwrap();
        let index = index.get_or_insert_with(build_desktop_index);
        index.get(&exe_name).cloned()
    };

    let result = icon.and_then(|icon| {
        if icon.starts_with('/') {
            let path = PathBuf::from(icon);
            path.is_file().then_some(path)
        } else {
            find_icon_in_theme(&icon)
        }
    });

    DESKTOP_CACHE
        .lock()
        .unwrap()
        .insert(exe_path.to_path_buf(), result.clone());
    result
}

fn desktop_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/applications"));
    }
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/flatpak/exports/share/applications"));
    }

    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| Path::new(dir).join("applications")),
    );
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));

    let mut seen = HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

fn desktop_exec_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;

    for character in command.chars() {
        if escaped {
            token.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                token.push(character);
            }
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(character);
        }
    }
    if escaped {
        token.push('\\');
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn executable_name(command: &str) -> Option<String> {
    let tokens = desktop_exec_tokens(command);
    let mut tokens = tokens.iter().map(String::as_str);
    let mut token = tokens.next()?;
    if Path::new(token)
        .file_name()
        .is_some_and(|name| name == "env")
    {
        token = tokens.find(|token| !token.contains('='))?;
    }
    Path::new(token)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
}

fn parse_desktop_entry(content: &str) -> Option<(Vec<String>, String)> {
    let mut in_desktop_entry = false;
    let mut executable_names = Vec::new();
    let mut icon = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            if in_desktop_entry {
                break;
            }
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry || line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix("Exec=")
            && let Some(name) = executable_name(value)
        {
            executable_names.push(name);
        } else if let Some(value) = line.strip_prefix("TryExec=")
            && let Some(name) = executable_name(value)
        {
            executable_names.push(name);
        } else if let Some(value) = line.strip_prefix("Icon=") {
            icon = Some(value.trim().to_owned());
        }
    }

    let icon = icon.filter(|icon| !icon.is_empty())?;
    (!executable_names.is_empty()).then_some((executable_names, icon))
}

fn build_desktop_index() -> HashMap<String, String> {
    let mut index = HashMap::new();
    for dir in desktop_search_dirs() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|extension| extension != "desktop")
            {
                continue;
            }
            let Ok(content) = fs::read_to_string(path) else {
                continue;
            };
            let Some((executables, icon)) = parse_desktop_entry(&content) else {
                continue;
            };
            for executable in executables {
                index.entry(executable).or_insert_with(|| icon.clone());
            }
        }
    }
    index
}

fn assemble_valid_ico(group: pelite::resources::group::GroupResource<'_>) -> Vec<u8> {
    let mut out = Vec::new();

    // Write ICO header
    out.extend_from_slice(&0u16.to_le_bytes()); // idReserved
    out.extend_from_slice(&1u16.to_le_bytes()); // idType

    let entries = group.entries();
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes()); // idCount

    let mut image_data = Vec::new();
    let mut image_offset = 6 + entries.len() as u32 * 16;

    for entry in entries {
        let bytes = group.image(entry.nId).unwrap_or(&[]);
        let actual_size = bytes.len() as u32;

        // Write ICONDIRENTRY
        out.push(entry.bWidth);
        out.push(entry.bHeight);
        out.push(entry.bColorCount);
        out.push(entry.bReserved);
        out.extend_from_slice(&entry.wPlanes.to_le_bytes());
        out.extend_from_slice(&entry.wBitCount.to_le_bytes());
        out.extend_from_slice(&actual_size.to_le_bytes());
        out.extend_from_slice(&image_offset.to_le_bytes());

        image_data.push(bytes);
        image_offset += actual_size;
    }

    // Append all image data
    for data in image_data {
        out.extend_from_slice(data);
    }

    out
}

fn extract_pe_icon_bytes(exe_path: &Path) -> Option<Vec<u8>> {
    let map = pelite::FileMap::open(exe_path).ok()?;
    let pe = pelite::PeFile::from_bytes(&map).ok()?;
    let resources = pe.resources().ok()?;

    for (_name, group) in resources.icons().filter_map(Result::ok) {
        let bytes = assemble_valid_ico(group);
        if !bytes.is_empty() {
            return Some(bytes);
        }
    }
    None
}

pub fn find_icon_via_pe(exe_path: &Path) -> Option<RawIcon> {
    let path_str = exe_path.to_string_lossy();
    if !path_str.to_lowercase().ends_with(".exe") {
        return None;
    }

    // Try the exact executable
    if let Some(bytes) = extract_pe_icon_bytes(exe_path) {
        return Some(RawIcon::PngOrIco(bytes.into()));
    }

    // Try sibling executables in the same directory, then parent directory
    let mut current_dir = exe_path.parent();
    for _ in 0..2 {
        if let Some(dir) = current_dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file()
                        && p.extension().is_some_and(|e| e == "exe")
                        && p != exe_path
                        && let Some(bytes) = extract_pe_icon_bytes(&p)
                    {
                        return Some(RawIcon::PngOrIco(bytes.into()));
                    }
                }
            }
            current_dir = dir.parent();
        } else {
            break;
        }
    }

    None
}

#[cfg(target_os = "linux")]
pub fn find_icon_via_appimage(exe_path: &Path) -> Option<RawIcon> {
    use backhand::{InnerNode, Squashfs};
    use memmap2::MmapOptions;
    use std::io::{Read, Seek, SeekFrom};

    let path_str = exe_path.to_string_lossy();
    if !path_str.to_lowercase().ends_with(".appimage") {
        return None;
    }

    let mut file = fs::File::open(exe_path).ok()?;
    let mmap = unsafe { MmapOptions::new().map(&file).ok()? };

    // Find squashfs magic 'hsqs'
    let offset = memchr::memmem::find(&mmap, b"hsqs")?;

    file.seek(SeekFrom::Start(offset as u64)).ok()?;

    let mut buf_reader = std::io::BufReader::new(file);
    let squashfs = Squashfs::from_reader(&mut buf_reader).ok()?;
    let fs_reader = squashfs.into_filesystem_reader().ok()?;

    let mut target_path = PathBuf::from("/.DirIcon");
    let mut resolved_node = None;

    for _ in 0..5 {
        let node = fs_reader.files().find(|n| n.fullpath == target_path);

        if let Some(n) = node {
            match &n.inner {
                InnerNode::Symlink(sym) => {
                    let link = PathBuf::from(&sym.link);
                    if link.has_root() {
                        target_path = link;
                    } else {
                        target_path = target_path
                            .parent()
                            .unwrap_or_else(|| Path::new("/"))
                            .join(link);
                    }
                }
                InnerNode::File(_) => {
                    resolved_node = Some(n.clone());
                    break;
                }
                _ => break,
            }
        } else {
            break;
        }
    }

    if let Some(n) = resolved_node
        && let InnerNode::File(file_node) = &n.inner
    {
        let mut reader = fs_reader.file(file_node).reader();
        let mut bytes = Vec::new();
        if reader.read_to_end(&mut bytes).is_ok() {
            let ext = target_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
                .to_lowercase();
            if ext == "svg" {
                return Some(RawIcon::Svg(bytes.into()));
            } else {
                return Some(RawIcon::PngOrIco(bytes.into()));
            }
        }
    }

    None
}

/// Release all icon-lookup caches. Call once after scan completes.
pub fn clear_icon_caches() {
    RAW_ICON_CACHE.lock().unwrap().clear();
    DESKTOP_CACHE.lock().unwrap().clear();
    *DESKTOP_INDEX.lock().unwrap() = None;
    ICON_THEME_CACHE.lock().unwrap().clear();
    NEIGHBOR_ICON_CACHE.lock().unwrap().clear();
}

pub fn get_app_icon(path: String) -> RawIcon {
    let exe_path = Path::new(&path);

    if let Some(cached) = RAW_ICON_CACHE.lock().unwrap().get(exe_path) {
        return cached.clone();
    }

    if let Some(b) = find_icon_via_pe(exe_path) {
        let result = b;
        RAW_ICON_CACHE
            .lock()
            .unwrap()
            .insert(exe_path.to_path_buf(), result.clone());
        return result;
    }

    #[cfg(target_os = "linux")]
    if let Some(b) = find_icon_via_appimage(exe_path) {
        let result = b;
        RAW_ICON_CACHE
            .lock()
            .unwrap()
            .insert(exe_path.to_path_buf(), result.clone());
        return result;
    }

    if let Some(p) = find_neighboring_icon(exe_path)
        && let Some(icon) = try_icon_from_path(&p)
    {
        RAW_ICON_CACHE
            .lock()
            .unwrap()
            .insert(exe_path.to_path_buf(), icon.clone());
        return icon;
    }

    #[cfg(target_os = "linux")]
    for path_finder in [
        |ep: &Path| find_icon_via_desktop_file(ep),
        |ep: &Path| crate::package_manager::find_icon_via_package_manager(ep),
    ] {
        if let Some(p) = path_finder(exe_path)
            && let Some(icon) = try_icon_from_path(&p)
        {
            RAW_ICON_CACHE
                .lock()
                .unwrap()
                .insert(exe_path.to_path_buf(), icon.clone());
            return icon;
        }
    }

    let icon = DEFAULT_ICON.clone();
    RAW_ICON_CACHE
        .lock()
        .unwrap()
        .insert(exe_path.to_path_buf(), icon.clone());
    icon
}

#[cfg(test)]
mod tests {
    use super::{desktop_exec_tokens, executable_name, parse_desktop_entry};

    #[test]
    fn desktop_exec_parser_handles_quotes_and_env() {
        assert_eq!(
            desktop_exec_tokens("env FOO=bar \"/opt/My App/app\" --flag %U"),
            ["env", "FOO=bar", "/opt/My App/app", "--flag", "%U"]
        );
        assert_eq!(
            executable_name("env FOO=bar \"/opt/My App/app\" --flag %U").as_deref(),
            Some("app")
        );
    }

    #[test]
    fn desktop_parser_stays_in_main_section() {
        let entry = "\
[Desktop Entry]\n\
Exec=/opt/example/bin/example %U\n\
TryExec=example\n\
Icon=example-icon\n\
[Desktop Action New]\n\
Exec=wrong\n\
Icon=wrong-icon\n";
        let (executables, icon) = parse_desktop_entry(entry).unwrap();
        assert_eq!(executables, ["example", "example"]);
        assert_eq!(icon, "example-icon");
    }
}
