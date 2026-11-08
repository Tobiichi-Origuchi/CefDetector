use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use crate::config::{IconConfig, IconSource};

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

#[cfg(target_os = "linux")]
static DESKTOP_CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(target_os = "linux")]
static DESKTOP_INDEX: LazyLock<Mutex<Option<HashMap<OsString, String>>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(target_os = "linux")]
static ICON_THEME_CACHE: LazyLock<Mutex<HashMap<String, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static NEIGHBOR_ICON_CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn read_icon_file(path: &Path, maximum: u64) -> Option<Vec<u8>> {
    let file = crate::file_io::open_regular_file(path).ok()?;
    if file.metadata().ok()?.len() > maximum {
        return None;
    }
    let mut bytes = Vec::new();
    // Bound the read even if the file grows after its metadata was checked.
    file.take(maximum + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > maximum {
        return None;
    }
    Some(bytes)
}

fn try_icon_from_path(path: &Path, maximum: u64) -> Option<RawIcon> {
    let bytes = read_icon_file(path, maximum)?;
    #[cfg(target_os = "macos")]
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("icns"))
    {
        return decode_icns(&bytes);
    }
    let bytes: Arc<[u8]> = bytes.into();
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

#[cfg(any(target_os = "macos", test))]
fn checked_icns_family(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(..4)? != b"icns" {
        return None;
    }
    let family_length = u32::from_be_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let family = bytes.get(..family_length)?;
    let mut elements = family.get(8..)?;
    while !elements.is_empty() {
        let element_length = u32::from_be_bytes(elements.get(4..8)?.try_into().ok()?) as usize;
        if element_length < 8 {
            return None;
        }
        elements = elements.get(element_length..)?;
    }
    Some(family)
}

#[cfg(target_os = "macos")]
fn decode_icns(bytes: &[u8]) -> Option<RawIcon> {
    // The parser allocates from element lengths before checking for truncated data.
    let family = icns::IconFamily::read(checked_icns_family(bytes)?).ok()?;
    let icon_type = family
        .available_icons()
        .into_iter()
        .max_by_key(|icon_type| {
            icon_type
                .pixel_width()
                .saturating_mul(icon_type.pixel_height())
        })?;
    let image = family.get_icon_with_type(icon_type).ok()?;
    let mut png = Vec::new();
    image.write_png(&mut png).ok()?;
    Some(RawIcon::PngOrIco(png.into()))
}

#[cfg(target_os = "linux")]
pub fn find_icon_in_theme(icon_name: &str, config: &IconConfig) -> Option<PathBuf> {
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

    let home_dirs: Option<[PathBuf; 5]> = std::env::var_os("HOME").map(|home| {
        [
            "/.local/share/icons/hicolor/512x512/apps",
            "/.local/share/icons/hicolor/256x256/apps",
            "/.local/share/icons/hicolor/128x128/apps",
            "/.local/share/icons/hicolor/scalable/apps",
            "/.local/share/icons",
        ]
        .map(|suffix| {
            // Preserve the original empty-HOME and trailing-separator behavior.
            let mut directory = home.clone();
            directory.push(suffix);
            PathBuf::from(directory)
        })
    });

    let result = {
        let mut found = None;
        for ext in ["png", "svg"] {
            for dir in config
                .linux
                .theme_directories
                .iter()
                .map(PathBuf::as_path)
                .chain(search_dirs.iter().map(Path::new))
            {
                let p = dir.join(format!("{}.{}", icon_name, ext));
                if p.exists() {
                    found = Some(p);
                    break;
                }
            }
            if found.is_none()
                && let Some(ref dirs) = home_dirs
            {
                for dir in dirs {
                    let p = dir.join(format!("{}.{}", icon_name, ext));
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

pub fn find_neighboring_icon(exe_path: &Path, config: &IconConfig) -> Option<PathBuf> {
    let parent = exe_path.parent()?;

    if let Some(cached) = NEIGHBOR_ICON_CACHE.lock().unwrap().get(parent) {
        return cached.clone();
    }

    let exe_name = exe_path.file_name()?;

    let result = (|| {
        let dirs_to_check = config
            .neighbor
            .directories
            .iter()
            .map(|directory| parent.join(directory));

        for dir in dirs_to_check {
            if !dir.is_dir() {
                continue;
            }
            for template in &config.neighbor.names {
                let mut name = OsString::new();
                for (index, part) in template.split("{executable}").enumerate() {
                    if index > 0 {
                        name.push(exe_name);
                    }
                    name.push(part);
                }
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

#[cfg(target_os = "linux")]
pub fn find_icon_via_desktop_file(exe_path: &Path, config: &IconConfig) -> Option<PathBuf> {
    if let Some(cached) = DESKTOP_CACHE.lock().unwrap().get(exe_path) {
        return cached.clone();
    }

    let exe_name = exe_path.file_name()?.to_ascii_lowercase();
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
            find_icon_in_theme(&icon, config)
        }
    });

    DESKTOP_CACHE
        .lock()
        .unwrap()
        .insert(exe_path.to_path_buf(), result.clone());
    result
}

#[cfg(target_os = "linux")]
fn desktop_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/applications"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/flatpak/exports/share/applications"));
    }

    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(
        std::env::split_paths(&data_dirs)
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(|dir| dir.join("applications")),
    );
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));

    let mut seen = HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn executable_name(command: &str) -> Option<OsString> {
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
        .map(|name| name.to_ascii_lowercase())
}

#[cfg(target_os = "linux")]
fn parse_desktop_entry(content: &str) -> Option<(Vec<OsString>, String)> {
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

#[cfg(target_os = "linux")]
fn build_desktop_index() -> HashMap<OsString, String> {
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

fn assemble_valid_ico(
    group: pelite::resources::group::GroupResource<'_>,
    maximum: u64,
) -> Option<Vec<u8>> {
    let entries = group.entries();
    let mut image_offset = 6 + entries.len() as u32 * 16;
    if u64::from(image_offset) > maximum {
        return None;
    }
    let mut out = Vec::new();

    // Write ICO header
    out.extend_from_slice(&0u16.to_le_bytes()); // idReserved
    out.extend_from_slice(&1u16.to_le_bytes()); // idType

    out.extend_from_slice(&(entries.len() as u16).to_le_bytes()); // idCount

    let mut image_data = Vec::new();

    for entry in entries {
        let bytes = group.image(entry.nId).unwrap_or(&[]);
        let actual_size = u32::try_from(bytes.len()).ok()?;
        let next_offset = image_offset.checked_add(actual_size)?;
        if u64::from(next_offset) > maximum {
            return None;
        }

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
        image_offset = next_offset;
    }

    // Append all image data
    for data in image_data {
        out.extend_from_slice(data);
    }

    Some(out)
}

fn extract_pe_icon_bytes(exe_path: &Path, config: &IconConfig) -> Option<Vec<u8>> {
    let bytes = read_icon_file(exe_path, config.max_pe_file_bytes)?;
    let pe = pelite::PeFile::from_bytes(&bytes).ok()?;
    let resources = pe.resources().ok()?;

    for (_name, group) in resources.icons().filter_map(Result::ok) {
        if let Some(bytes) = assemble_valid_ico(group, config.max_pe_icon_bytes) {
            return Some(bytes);
        }
    }
    None
}

pub fn find_icon_via_pe(exe_path: &Path, config: &IconConfig) -> Option<RawIcon> {
    let path_str = exe_path.to_string_lossy();
    if !path_str.to_lowercase().ends_with(".exe") {
        return None;
    }

    // Try the exact executable
    if let Some(bytes) = extract_pe_icon_bytes(exe_path, config) {
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
                        && let Some(bytes) = extract_pe_icon_bytes(&p, config)
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
fn find_squashfs_offset(mut reader: impl std::io::Read) -> Option<u64> {
    let mut buffer = [0; 8192];
    let mut retained = 0;
    let mut offset = 0;
    let finder = memchr::memmem::Finder::new(b"hsqs");
    loop {
        let count = match reader.read(&mut buffer[retained..]) {
            Ok(0) => return None,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let end = retained + count;
        if let Some(position) = finder.find(&buffer[..end]) {
            return Some(offset + position as u64);
        }
        // Keep partial matches across read boundaries.
        retained = end.min(3);
        buffer.copy_within(end - retained..end, 0);
        offset += (end - retained) as u64;
    }
}

#[cfg(target_os = "linux")]
pub fn find_icon_via_appimage(exe_path: &Path, maximum: u64) -> Option<RawIcon> {
    use backhand::{InnerNode, Squashfs};
    use std::io::{Read, Seek, SeekFrom};

    let path_str = exe_path.to_string_lossy();
    if !path_str.to_lowercase().ends_with(".appimage") {
        return None;
    }

    let mut file = crate::file_io::open_regular_file(exe_path).ok()?;
    let length = file.metadata().ok()?.len();
    let offset = find_squashfs_offset((&mut file).take(length))?;

    file.seek(SeekFrom::Start(offset)).ok()?;

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
        if file_node.file_len() as u64 > maximum {
            return None;
        }
        let mut reader = fs_reader.file(file_node).reader().take(maximum + 1);
        let mut bytes = Vec::new();
        if reader.read_to_end(&mut bytes).is_ok() && bytes.len() as u64 <= maximum {
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
    NEIGHBOR_ICON_CACHE.lock().unwrap().clear();
    #[cfg(target_os = "linux")]
    {
        DESKTOP_CACHE.lock().unwrap().clear();
        *DESKTOP_INDEX.lock().unwrap() = None;
        ICON_THEME_CACHE.lock().unwrap().clear();
    }
}

pub fn configured_fallback_icon(config: &IconConfig) -> RawIcon {
    if !config.enabled || !config.sources.contains(&IconSource::Builtin) {
        return RawIcon::Empty;
    }
    config
        .fallback_path
        .as_deref()
        .and_then(|path| try_icon_from_path(path, config.max_file_bytes))
        .unwrap_or_else(|| DEFAULT_ICON.clone())
}

pub fn get_app_icon(exe_path: &Path, config: &IconConfig) -> RawIcon {
    if !config.enabled {
        return RawIcon::Empty;
    }
    if let Some(cached) = RAW_ICON_CACHE.lock().unwrap().get(exe_path) {
        return cached.clone();
    }

    let mut icon = None;
    for source in &config.sources {
        icon = match source {
            IconSource::Bundle => {
                #[cfg(target_os = "macos")]
                {
                    crate::search::macos::bundle_icon_path(exe_path)
                        .as_deref()
                        .and_then(|path| try_icon_from_path(path, config.max_file_bytes))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    None
                }
            }
            IconSource::Pe => find_icon_via_pe(exe_path, config),
            IconSource::Appimage => {
                #[cfg(target_os = "linux")]
                {
                    find_icon_via_appimage(exe_path, config.max_appimage_icon_bytes)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    None
                }
            }
            IconSource::Neighbor => find_neighboring_icon(exe_path, config)
                .as_deref()
                .and_then(|path| try_icon_from_path(path, config.max_file_bytes)),
            IconSource::Desktop => {
                #[cfg(target_os = "linux")]
                {
                    config
                        .linux
                        .desktop_files
                        .then(|| find_icon_via_desktop_file(exe_path, config))
                        .flatten()
                        .as_deref()
                        .and_then(|path| try_icon_from_path(path, config.max_file_bytes))
                }
                #[cfg(not(target_os = "linux"))]
                {
                    None
                }
            }
            IconSource::PackageManager => {
                #[cfg(target_os = "linux")]
                {
                    config
                        .linux
                        .package_managers
                        .then(|| {
                            crate::package_manager::find_icon_via_package_manager(
                                exe_path,
                                &config.linux.package_query,
                            )
                        })
                        .flatten()
                        .as_deref()
                        .and_then(|path| try_icon_from_path(path, config.max_file_bytes))
                }
                #[cfg(not(target_os = "linux"))]
                {
                    None
                }
            }
            IconSource::Builtin => Some(configured_fallback_icon(config)),
        };
        if icon.is_some() {
            break;
        }
    }

    let icon = icon.unwrap_or(RawIcon::Empty);
    RAW_ICON_CACHE
        .lock()
        .unwrap()
        .insert(exe_path.to_path_buf(), icon.clone());
    icon
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use std::fs;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("cefdetector-icon-{name}-{}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn neighboring_templates_preserve_native_executable_names() {
        #[cfg(target_os = "linux")]
        let names = {
            use std::os::unix::ffi::OsStringExt as _;
            [
                OsString::from_vec(b"app-\xff".to_vec()),
                OsString::from_vec(b"app-\xfe".to_vec()),
            ]
        };
        #[cfg(windows)]
        let names = {
            use std::os::windows::ffi::OsStringExt as _;
            [
                OsString::from_wide(&[b'a' as u16, 0xD800]),
                OsString::from_wide(&[b'a' as u16, 0xDC00]),
            ]
        };

        let root = TempDirectory::new("native-neighbor-template");
        let config = IconConfig {
            sources: vec![IconSource::Neighbor],
            neighbor: crate::config::NeighborIconConfig {
                directories: vec!["missing".into(), ".".into(), "art".into()],
                names: vec![
                    "missing-{executable}.png".into(),
                    "prefix-{executable}-{executable}.png".into(),
                    "fallback.png".into(),
                ],
            },
            ..Default::default()
        };
        let image = include_bytes!("../icons/32x32.png");
        for (index, name) in names.into_iter().enumerate() {
            assert!(name.to_str().is_none());
            let directory = root.0.join(index.to_string());
            fs::create_dir(&directory).unwrap();
            fs::create_dir(directory.join("art")).unwrap();
            let executable = directory.join(&name);
            fs::write(&executable, []).unwrap();
            let mut icon_name = OsString::from("prefix-");
            icon_name.push(&name);
            icon_name.push("-");
            icon_name.push(&name);
            icon_name.push(".png");
            let icon = directory.join(&icon_name);
            fs::write(&icon, image).unwrap();
            let decoy = directory.join(icon_name.to_string_lossy().as_ref());
            assert_ne!(icon, decoy);
            fs::write(&decoy, include_bytes!("../icons/128x128.png")).unwrap();
            fs::write(directory.join("fallback.png"), []).unwrap();
            fs::write(directory.join("art").join(icon_name), []).unwrap();

            assert_eq!(find_neighboring_icon(&executable, &config), Some(icon));
            let RawIcon::PngOrIco(bytes) = get_app_icon(&executable, &config) else {
                panic!("native executable name lost its neighboring icon");
            };
            assert_eq!(&*bytes, image);
        }
    }

    fn pe_with_icon_groups(image: &[u8], counts: &[u16]) -> Vec<u8> {
        fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        fn directory(bytes: &mut [u8], offset: usize, entries: &[(u32, u32)]) {
            put_u16(bytes, offset + 14, entries.len() as u16);
            for (index, &(name, target)) in entries.iter().enumerate() {
                put_u32(bytes, offset + 16 + index * 8, name);
                put_u32(bytes, offset + 20 + index * 8, target);
            }
        }

        // One RT_ICON image is shared by all entries in every RT_GROUP_ICON.
        let image_offset = 112 + counts.len() * 48;
        let mut section = vec![0; image_offset];
        directory(&mut section, 0, &[(3, 0x8000_0020), (14, 0x8000_0060)]);
        directory(&mut section, 32, &[(1, 0x8000_0038)]);
        directory(&mut section, 56, &[(1033, 80)]);
        put_u32(&mut section, 80, 0x1000 + image_offset as u32);
        put_u32(&mut section, 84, image.len() as u32);
        let groups: Vec<_> = (0..counts.len())
            .map(|index| {
                (
                    index as u32 + 1,
                    0x8000_0000 | (112 + counts.len() * 8 + index * 40) as u32,
                )
            })
            .collect();
        directory(&mut section, 96, &groups);
        section.extend_from_slice(image);
        section.resize(section.len().next_multiple_of(4), 0);
        for (index, &count) in counts.iter().enumerate() {
            let language_offset = 112 + counts.len() * 8 + index * 40;
            directory(
                &mut section,
                language_offset,
                &[(1033, (language_offset + 24) as u32)],
            );
            let data_offset = section.len() as u32;
            put_u32(&mut section, language_offset + 24, 0x1000 + data_offset);
            put_u32(
                &mut section,
                language_offset + 28,
                6 + u32::from(count) * 14,
            );
            section.extend_from_slice(&[0, 0, 1, 0]);
            section.extend_from_slice(&count.to_le_bytes());
            for _ in 0..count {
                section.extend_from_slice(&[128, 128, 0, 0, 1, 0, 32, 0]);
                section.extend_from_slice(&(image.len() as u32).to_le_bytes());
                section.extend_from_slice(&1_u16.to_le_bytes());
            }
        }

        // A PE32+ file with a single resource section. It is parsed, never run.
        let mut pe = vec![0; 512];
        pe[..2].copy_from_slice(b"MZ");
        put_u32(&mut pe, 60, 128);
        pe[128..132].copy_from_slice(b"PE\0\0");
        put_u16(&mut pe, 132, 0x8664);
        put_u16(&mut pe, 134, 1);
        put_u16(&mut pe, 148, 240);
        put_u16(&mut pe, 150, 0x22);
        let optional = 152;
        put_u16(&mut pe, optional, 0x20B);
        pe[optional + 24..optional + 32].copy_from_slice(&0x1_4000_0000_u64.to_le_bytes());
        put_u32(&mut pe, optional + 32, 0x1000);
        put_u32(&mut pe, optional + 36, 512);
        put_u32(
            &mut pe,
            optional + 56,
            (0x1000 + section.len()).next_multiple_of(4096) as u32,
        );
        put_u32(&mut pe, optional + 60, 512);
        put_u16(&mut pe, optional + 68, 3);
        put_u32(&mut pe, optional + 108, 16);
        put_u32(&mut pe, optional + 128, 0x1000);
        put_u32(&mut pe, optional + 132, section.len() as u32);
        let raw_size = section.len().next_multiple_of(512);
        let header = optional + 240;
        pe[header..header + 8].copy_from_slice(b".rsrc\0\0\0");
        put_u32(&mut pe, header + 8, section.len() as u32);
        put_u32(&mut pe, header + 12, 0x1000);
        put_u32(&mut pe, header + 16, raw_size as u32);
        put_u32(&mut pe, header + 20, 512);
        put_u32(&mut pe, header + 36, 0x4000_0040);
        pe.extend_from_slice(&section);
        pe.resize(512 + raw_size, 0);
        pe
    }

    #[test]
    fn pe_icon_byte_limits_include_headers_and_repeated_images() {
        let mut image = include_bytes!("../icons/128x128.png").to_vec();
        image.resize(65536, 0);
        for count in [0, 1, 2, 144] {
            let bytes = pe_with_icon_groups(&image, &[count]);
            let pe = pelite::PeFile::from_bytes(&bytes).unwrap();
            let resources = pe.resources().unwrap();
            let (_, group) = resources.icons().next().unwrap().unwrap();
            let expected = 6 + u64::from(count) * (16 + image.len() as u64);
            for maximum in [1, expected - 1, expected, expected + 1] {
                match assemble_valid_ico(group, maximum) {
                    None => assert!(maximum < expected, "{count}: {maximum}/{expected}"),
                    Some(icon) => {
                        assert!(maximum >= expected, "{count}: {maximum}/{expected}");
                        assert_eq!(icon.len() as u64, expected);
                        assert_eq!(u16::from_le_bytes(icon[4..6].try_into().unwrap()), count);
                        assert!(
                            icon[6 + usize::from(count) * 16..]
                                .chunks_exact(image.len())
                                .all(|chunk| chunk == image)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn pe_lookup_skips_oversized_groups_and_uses_its_own_limit() {
        let root = TempDirectory::new("pe-group-limit");
        let path = root.0.join("groups.exe");
        let mut image = include_bytes!("../icons/128x128.png").to_vec();
        image.resize(65536, 0);
        fs::write(&path, pe_with_icon_groups(&image, &[144, 1])).unwrap();
        let maximum = 6 + 16 + image.len() as u64;
        assert!(fs::metadata(&path).unwrap().len() > maximum);
        let config = IconConfig {
            max_file_bytes: 1,
            max_appimage_icon_bytes: 1,
            max_pe_icon_bytes: maximum,
            sources: vec![IconSource::Pe],
            ..Default::default()
        };
        let RawIcon::PngOrIco(bytes) = get_app_icon(&path, &config) else {
            panic!("an oversized first PE group blocked the smaller group");
        };
        assert_eq!(bytes.len() as u64, maximum);
        assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()), 1);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (128, 128));
    }

    #[test]
    fn pe_container_reads_respect_the_independent_file_limit() {
        let root = TempDirectory::new("pe-file-limit");
        let path = root.0.join("app.exe");
        let image = include_bytes!("../icons/128x128.png");
        let mut file_bytes = pe_with_icon_groups(image, &[1]);
        file_bytes.resize(32768, 0); // A valid image with an unused file overlay.
        fs::write(&path, &file_bytes).unwrap();
        let file_size = file_bytes.len() as u64;
        let icon_size = 6 + 16 + image.len() as u64;
        for maximum in [
            1,
            file_size - 1,
            file_size,
            file_size + 1,
            4 * 1024 * 1024 * 1024,
        ] {
            let config = IconConfig {
                max_file_bytes: 1,
                max_appimage_icon_bytes: 1,
                max_pe_icon_bytes: icon_size,
                max_pe_file_bytes: maximum,
                ..Default::default()
            };
            match extract_pe_icon_bytes(&path, &config) {
                None => assert!(maximum < file_size, "{maximum}/{file_size}"),
                Some(bytes) => {
                    assert!(maximum >= file_size, "{maximum}/{file_size}");
                    assert_eq!(bytes.len() as u64, icon_size);
                    let decoded = image::load_from_memory(&bytes).unwrap();
                    assert_eq!((decoded.width(), decoded.height()), (128, 128));
                }
            }
        }
    }

    #[test]
    fn pe_icon_rejections_continue_the_existing_lookup_chain() {
        let root = TempDirectory::new("pe-limit-fallback");
        let mut image = include_bytes!("../icons/128x128.png").to_vec();
        image.resize(65536, 0);
        let oversized = pe_with_icon_groups(&image, &[144]);
        let small = pe_with_icon_groups(&image, &[1]);
        for rejection in ["icon", "file"] {
            for name in ["sibling", "parent", "neighbor", "builtin", "empty"] {
                let directory = root.0.join(rejection).join(name);
                let bin = directory.join("bin");
                fs::create_dir_all(&bin).unwrap();
                let path = bin.join("app.exe");
                fs::write(&path, &oversized).unwrap();
                let mut config = IconConfig {
                    max_pe_icon_bytes: 6 + 16 + image.len() as u64,
                    sources: vec![IconSource::Pe],
                    ..Default::default()
                };
                if rejection == "file" {
                    config.max_pe_file_bytes = small.len() as u64;
                    config.max_pe_icon_bytes = 6 + 144 * (16 + image.len() as u64);
                }
                match name {
                    "sibling" => fs::write(bin.join("other.exe"), &small).unwrap(),
                    "parent" => fs::write(directory.join("other.exe"), &small).unwrap(),
                    "neighbor" => {
                        fs::write(bin.join("icon.png"), include_bytes!("../icons/128x128.png"))
                            .unwrap();
                        config.neighbor.names = vec!["icon.png".into()];
                        config.sources.push(IconSource::Neighbor);
                    }
                    "builtin" => config.sources.push(IconSource::Builtin),
                    "empty" => {}
                    _ => unreachable!(),
                }
                let actual = get_app_icon(&path, &config);
                match (name, actual) {
                    ("sibling" | "parent", RawIcon::PngOrIco(bytes)) => {
                        assert_eq!(bytes.len() as u64, 6 + 16 + image.len() as u64);
                        assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()), 1);
                    }
                    ("neighbor", RawIcon::PngOrIco(bytes)) => {
                        assert_eq!(&*bytes, include_bytes!("../icons/128x128.png"));
                    }
                    ("builtin", RawIcon::PngOrIco(bytes)) => {
                        assert_eq!(&*bytes, include_bytes!("../icons/default_cef_icon.ico"));
                    }
                    ("empty", RawIcon::Empty) => {}
                    _ => panic!("unexpected source result for {name}"),
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_icon_lookup_rejects_special_files_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;
        use std::process::Command;
        use std::time::{Duration, Instant};

        const CHILD_ROOT: &str = "CEFDETECTOR_TEST_APPIMAGE_SPECIAL_FILES";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = Path::new(&root);
            let config = IconConfig {
                sources: vec![IconSource::Appimage, IconSource::Builtin],
                ..Default::default()
            };
            for name in [
                "fifo.AppImage",
                "link.AppImage",
                "socket.AppImage",
                "directory.AppImage",
                "missing.AppImage",
            ] {
                let path = root.join(name);
                assert!(
                    find_icon_via_appimage(&path, config.max_appimage_icon_bytes).is_none(),
                    "{name}"
                );
                let RawIcon::PngOrIco(bytes) = get_app_icon(&path, &config) else {
                    panic!("{name} did not continue to the builtin source");
                };
                assert_eq!(&*bytes, include_bytes!("../icons/default_cef_icon.ico"));
            }
            return;
        }

        let root = TempDirectory::new("appimage-special-files");
        let fifo = root.0.join("fifo.AppImage");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: fifo_c is a valid, NUL-terminated path owned by this test.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        symlink(&fifo, root.0.join("link.AppImage")).unwrap();
        let _listener = UnixListener::bind(root.0.join("socket.AppImage")).unwrap();
        fs::create_dir(root.0.join("directory.AppImage")).unwrap();
        // Keep an accidental blocking open from hanging the test runner itself.
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "icon_finder::input_tests::appimage_icon_lookup_rejects_special_files_without_blocking",
            ])
            .env(CHILD_ROOT, &root.0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("AppImage icon lookup blocked on a special file");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(target_os = "linux")]
    fn write_appimage_icon(path: &Path, name: &str, bytes: &[u8]) {
        use backhand::{FilesystemWriter, NodeHeader};
        use std::io::Cursor;

        let mut writer = FilesystemWriter::default();
        writer
            .push_file(
                Cursor::new(bytes),
                format!("/{name}"),
                NodeHeader::default(),
            )
            .unwrap();
        writer
            .push_symlink(name, "/.DirIcon", NodeHeader::default())
            .unwrap();
        writer.write(fs::File::create(path).unwrap()).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_icon_lookup_preserves_regular_files_and_symlinks() {
        use std::os::unix::fs::symlink;

        let root = TempDirectory::new("appimage-regular-files");
        let expected = include_bytes!("../icons/default_cef_icon.ico");
        let path = root.0.join("regular.AppImage");
        write_appimage_icon(&path, "icon.ico", expected);
        let link = root.0.join("link.appimage");
        symlink(&path, &link).unwrap();
        for path in [&path, &link] {
            let Some(RawIcon::PngOrIco(bytes)) =
                find_icon_via_appimage(path, IconConfig::default().max_appimage_icon_bytes)
            else {
                panic!("{} did not produce an icon", path.display());
            };
            assert_eq!(&*bytes, expected);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_icon_reads_respect_the_extracted_byte_limit() {
        let root = TempDirectory::new("appimage-byte-limit");
        let path = root.0.join("limited.AppImage");
        for name in ["icon.ico", "icon.svg"] {
            for maximum in [1, 16, IconConfig::default().max_appimage_icon_bytes] {
                for length in [maximum - 1, maximum, maximum + 1] {
                    let expected = vec![b'x'; length as usize];
                    write_appimage_icon(&path, name, &expected);
                    match find_icon_via_appimage(&path, maximum) {
                        None => assert!(length > maximum, "{name}: {length}/{maximum}"),
                        Some(icon) => {
                            assert!(length <= maximum, "{name}: {length}/{maximum}");
                            let bytes = match icon {
                                RawIcon::Svg(bytes) if name.ends_with(".svg") => bytes,
                                RawIcon::PngOrIco(bytes) if name.ends_with(".ico") => bytes,
                                _ => panic!("unexpected icon kind for {name}"),
                            };
                            assert_eq!(&*bytes, expected);
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_limits_are_independent_and_oversized_icons_follow_existing_sources() {
        let root = TempDirectory::new("appimage-limit-fallback");
        let icon = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="red"/></svg>"#;
        let neighbor = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="blue"/></svg>"#;
        fs::write(root.0.join("icon.svg"), neighbor).unwrap();
        let mut config = IconConfig {
            max_file_bytes: neighbor.len() as u64,
            max_appimage_icon_bytes: icon.len() as u64 - 1,
            ..Default::default()
        };
        config.neighbor.names = vec!["icon.svg".into()];
        for (name, sources) in [
            ("neighbor", vec![IconSource::Appimage, IconSource::Neighbor]),
            ("builtin", vec![IconSource::Appimage, IconSource::Builtin]),
            ("empty", vec![IconSource::Appimage]),
            ("within-limit", vec![IconSource::Appimage]),
        ] {
            let path = root.0.join(format!("{name}.AppImage"));
            write_appimage_icon(&path, "icon.svg", icon);
            config.sources = sources;
            if name == "within-limit" {
                config.max_file_bytes = 1;
                config.max_appimage_icon_bytes = icon.len() as u64;
            }
            let actual = get_app_icon(&path, &config);
            match (name, actual) {
                ("neighbor", RawIcon::Svg(bytes)) => assert_eq!(&*bytes, neighbor),
                ("builtin", RawIcon::PngOrIco(bytes)) => {
                    assert_eq!(&*bytes, include_bytes!("../icons/default_cef_icon.ico"));
                }
                ("empty", RawIcon::Empty) => {}
                ("within-limit", RawIcon::Svg(bytes)) => assert_eq!(&*bytes, icon),
                _ => panic!("unexpected source result for {name}"),
            }
        }
    }

    #[test]
    fn icns_length_checks_reject_truncated_and_oversized_elements() {
        for length in 0..8 {
            assert!(checked_icns_family(&b"icns\0\0\0\x08"[..length]).is_none());
        }
        assert!(checked_icns_family(b"nope\0\0\0\x08").is_none());
        for family_length in [0_u32, 1, 7, 9, 15, 17, u32::MAX] {
            let mut bytes = b"icns".to_vec();
            bytes.extend_from_slice(&family_length.to_be_bytes());
            bytes.extend_from_slice(b"quux\0\0\0\x08");
            assert!(checked_icns_family(&bytes).is_none(), "{family_length}");
        }
        for element_length in [0_u32, 1, 7, 9, 128 * 1024 * 1024, u32::MAX] {
            let mut bytes = b"icns\0\0\0\x10quux".to_vec();
            bytes.extend_from_slice(&element_length.to_be_bytes());
            assert!(checked_icns_family(&bytes).is_none(), "{element_length}");
        }
        // An element must fit inside the declared family, even with extra input.
        assert!(checked_icns_family(b"icns\0\0\0\x10quux\0\0\0\x09#").is_none());
        // Check later elements as well as the first one.
        assert!(checked_icns_family(b"icns\0\0\0\x18quux\0\0\0\x08icp5\xff\xff\xff\xff").is_none());
    }

    #[test]
    fn icns_length_checks_preserve_complete_families_and_trailing_bytes() {
        let empty = b"icns\0\0\0\x08";
        assert_eq!(checked_icns_family(empty), Some(empty.as_slice()));
        let bytes = b"icns\0\0\0\x1fquux\0\0\0\x0efoobarbaz!\0\0\0\x09#";
        assert_eq!(checked_icns_family(bytes), Some(bytes.as_slice()));
        let mut padded = bytes.to_vec();
        padded.extend_from_slice(b"padding after the declared family");
        assert_eq!(checked_icns_family(&padded), Some(bytes.as_slice()));
    }

    #[test]
    fn icon_file_reads_respect_the_input_byte_limit() {
        let root = TempDirectory::new("input-limit");
        let path = root.0.join("icon.png");
        let file = fs::File::create(&path).unwrap();
        for maximum in [1, 16, IconConfig::default().max_file_bytes] {
            for (length, expected) in [
                (0, Some(0)),
                (maximum - 1, Some(maximum - 1)),
                (maximum, Some(maximum)),
                (maximum + 1, None),
                (maximum * 8, None),
            ] {
                file.set_len(length).unwrap();
                assert_eq!(
                    read_icon_file(&path, maximum).map(|bytes| bytes.len() as u64),
                    expected,
                    "input length: {length}, limit: {maximum}"
                );
            }
        }
    }

    #[test]
    fn oversized_icon_files_follow_the_existing_fallback_policy() {
        let root = TempDirectory::new("input-fallback");
        let icon_path = root.0.join("icon.png");
        fs::File::create(&icon_path).unwrap().set_len(17).unwrap();
        let mut config = IconConfig {
            max_file_bytes: 16,
            sources: vec![IconSource::Neighbor, IconSource::Builtin],
            ..Default::default()
        };
        let app = root.0.join("with-builtin");
        fs::write(&app, []).unwrap();
        let actual = get_app_icon(&app, &config);
        let RawIcon::PngOrIco(bytes) = actual else {
            panic!("oversized neighboring icon did not use the builtin fallback");
        };
        assert_eq!(&*bytes, include_bytes!("../icons/default_cef_icon.ico"));

        config.fallback_path = Some(icon_path);
        let RawIcon::PngOrIco(bytes) = configured_fallback_icon(&config) else {
            panic!("oversized configured fallback did not use the embedded icon");
        };
        assert_eq!(&*bytes, include_bytes!("../icons/default_cef_icon.ico"));

        config.sources = vec![IconSource::Neighbor];
        let app = root.0.join("without-builtin");
        fs::write(&app, []).unwrap();
        assert!(matches!(get_app_icon(&app, &config), RawIcon::Empty));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{
        RawIcon, clear_icon_caches, configured_fallback_icon, desktop_exec_tokens, executable_name,
        find_neighboring_icon, find_squashfs_offset, parse_desktop_entry,
    };

    #[test]
    fn theme_icons_preserve_native_home_paths() {
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::ffi::OsStringExt as _;
        use std::path::PathBuf;
        use std::process::Command;

        const CHILD_HOME: &str = "CEFDETECTOR_TEST_THEME_NATIVE_HOME";
        if let Some(home_path) = std::env::var_os(CHILD_HOME) {
            let home_path = PathBuf::from(home_path);
            let dirs = [
                ".local/share/icons/hicolor/512x512/apps",
                ".local/share/icons/hicolor/256x256/apps",
                ".local/share/icons/hicolor/128x128/apps",
                ".local/share/icons/hicolor/scalable/apps",
                ".local/share/icons",
            ]
            .map(|suffix| home_path.join(suffix));
            let prefix = format!("cefdetector-theme-test-{}", std::process::id());
            let png = include_bytes!("../icons/32x32.png");
            let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
            let mut config = crate::config::IconConfig::default();
            for (index, dir) in dirs.iter().enumerate() {
                fs::create_dir_all(dir).unwrap();
                let name = format!("{prefix}-directory-{index}");
                let icon = dir.join(format!("{name}.png"));
                fs::write(&icon, png).unwrap();
                assert_eq!(super::find_icon_in_theme(&name, &config), Some(icon));
            }
            let name = format!("{prefix}-home-order");
            for dir in &dirs {
                fs::write(dir.join(format!("{name}.png")), png).unwrap();
            }
            assert_eq!(
                super::find_icon_in_theme(&name, &config),
                Some(dirs[0].join(format!("{name}.png")))
            );
            let configured = home_path.join("configured-theme");
            fs::create_dir(&configured).unwrap();
            config.linux.theme_directories.push(configured.clone());
            let name = format!("{prefix}-configured");
            for dir in [&configured, &dirs[0]] {
                fs::write(dir.join(format!("{name}.png")), png).unwrap();
            }
            assert_eq!(
                super::find_icon_in_theme(&name, &config),
                Some(configured.join(format!("{name}.png")))
            );
            let name = format!("{prefix}-png-before-svg");
            fs::write(configured.join(format!("{name}.svg")), svg).unwrap();
            let icon = dirs[1].join(format!("{name}.png"));
            fs::write(&icon, png).unwrap();
            assert_eq!(super::find_icon_in_theme(&name, &config), Some(icon));
            let name = format!("{prefix}-svg");
            let icon = dirs[4].join(format!("{name}.svg"));
            fs::write(&icon, svg).unwrap();
            assert_eq!(super::find_icon_in_theme(&name, &config), Some(icon));

            let name = format!("{prefix}-desktop");
            fs::write(dirs[0].join(format!("{name}.png")), png).unwrap();
            let content = format!("[Desktop Entry]\nExec={prefix}\nIcon={name}\n");
            let (executables, icon) = parse_desktop_entry(&content).unwrap();
            *super::DESKTOP_INDEX.lock().unwrap() = Some(
                executables
                    .into_iter()
                    .map(|exe| (exe, icon.clone()))
                    .collect(),
            );
            config.sources = vec![
                crate::config::IconSource::Desktop,
                crate::config::IconSource::Builtin,
            ];
            let executable = home_path.join(&prefix);
            fs::write(&executable, []).unwrap();
            let RawIcon::PngOrIco(bytes) = super::get_app_icon(&executable, &config) else {
                panic!("native HOME lost its desktop theme icon");
            };
            assert_eq!(&*bytes, png);
            return;
        }

        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let root = Fixture(
            std::env::temp_dir().join(format!("cefdetector-theme-native-{}", std::process::id())),
        );
        fs::create_dir(&root.0).unwrap();
        let mut failures = Vec::new();
        for name in [
            OsString::from_vec(b"home-\xff".to_vec()),
            OsString::from_vec(b"home-\xfe".to_vec()),
            OsString::from("home-�"),
            OsString::from("home-中文"),
        ] {
            let home_path = root.0.join(&name);
            fs::create_dir(&home_path).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "icon_finder::tests::theme_icons_preserve_native_home_paths",
                ])
                .env(CHILD_HOME, &home_path)
                .env("HOME", &home_path)
                .output()
                .unwrap();
            if !output.status.success() {
                failures.push(format!(
                    "{name:?}:\n{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn desktop_search_directories_preserve_native_environment_paths() {
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
        use std::path::PathBuf;
        use std::process::Command;

        const EXPECTED_FILE: &str = "CEFDETECTOR_TEST_DESKTOP_DIRS_EXPECTED";
        if let Some(expected_file) = std::env::var_os(EXPECTED_FILE) {
            let expected_file = PathBuf::from(expected_file);
            let bytes = fs::read(&expected_file).unwrap();
            let expected: Vec<_> = bytes
                .strip_suffix(b"\0")
                .unwrap()
                .split(|byte| *byte == 0)
                .map(|bytes| PathBuf::from(OsString::from_vec(bytes.to_vec())))
                .collect();
            assert_eq!(super::desktop_search_dirs(), expected);
            for directory in expected
                .iter()
                .filter(|dir| dir.starts_with(expected_file.parent().unwrap()))
            {
                assert!(directory.join("native.desktop").is_file());
            }
            return;
        }

        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let root = Fixture(
            std::env::temp_dir().join(format!("cefdetector-desktop-dirs-{}", std::process::id())),
        );
        fs::create_dir(&root.0).unwrap();
        let native_home = root.0.join(OsString::from_vec(b"home-\xff".to_vec()));
        let native_data_home = root.0.join(OsString::from_vec(b"data-\xfe".to_vec()));
        let native_global = root.0.join(OsString::from_vec(b"global-\xff".to_vec()));
        let other_native_global = root.0.join(OsString::from_vec(b"global-\xfe".to_vec()));
        let home_path = root.0.join("home-中文");
        let data_home_path = root.0.join("data-中文");
        let global_path = root.0.join("global with space \"quote\"");
        let mut native_dirs = OsString::from(":");
        native_dirs.push(&native_global);
        native_dirs.push("::");
        native_dirs.push(&other_native_global);
        native_dirs.push(":");
        native_dirs.push(&global_path);
        native_dirs.push(":");
        native_dirs.push(&native_global);
        native_dirs.push(":");
        let mut unicode_dirs = OsString::from(":");
        unicode_dirs.push(&data_home_path);
        unicode_dirs.push("::");
        unicode_dirs.push(&global_path);
        unicode_dirs.push(":");
        unicode_dirs.push(&data_home_path);
        unicode_dirs.push(":");
        let flatpak_apps = ".local/share/flatpak/exports/share/applications";
        let cases = [
            (
                "home-native",
                Some(native_home.clone().into_os_string()),
                None,
                Some(global_path.clone().into_os_string()),
                vec![
                    native_home.join(".local/share/applications"),
                    native_home.join(flatpak_apps),
                    global_path.join("applications"),
                ],
            ),
            (
                "data-home-native",
                Some(home_path.clone().into_os_string()),
                Some(native_data_home.clone().into_os_string()),
                Some(global_path.clone().into_os_string()),
                vec![
                    native_data_home.join("applications"),
                    home_path.join(flatpak_apps),
                    global_path.join("applications"),
                ],
            ),
            (
                "data-dirs-native",
                None,
                None,
                Some(native_dirs),
                vec![
                    native_global.join("applications"),
                    other_native_global.join("applications"),
                    global_path.join("applications"),
                ],
            ),
            (
                "unicode",
                Some(home_path.clone().into_os_string()),
                Some(data_home_path.clone().into_os_string()),
                Some(unicode_dirs),
                vec![
                    data_home_path.join("applications"),
                    home_path.join(flatpak_apps),
                    global_path.join("applications"),
                ],
            ),
            (
                "unset",
                None,
                None,
                None,
                vec![
                    PathBuf::from("/usr/local/share/applications"),
                    PathBuf::from("/usr/share/applications"),
                ],
            ),
        ];
        let mut failures = Vec::new();
        for (label, home_value, data_home_value, data_dirs_value, mut expected) in cases {
            expected.extend([
                PathBuf::from("/var/lib/flatpak/exports/share/applications"),
                PathBuf::from("/var/lib/snapd/desktop/applications"),
            ]);
            let mut bytes = Vec::new();
            for directory in &expected {
                if directory.starts_with(&root.0) {
                    fs::create_dir_all(directory).unwrap();
                    fs::write(
                        directory.join("native.desktop"),
                        "[Desktop Entry]\nExec=native-path-fixture\nIcon=fixture\n",
                    )
                    .unwrap();
                }
                bytes.extend_from_slice(directory.as_os_str().as_bytes());
                bytes.push(0);
            }
            let expected_file = root.0.join(format!("{label}.expected"));
            fs::write(&expected_file, bytes).unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "icon_finder::tests::desktop_search_directories_preserve_native_environment_paths",
            ]).env(EXPECTED_FILE, &expected_file);
            for (key, value) in [
                ("HOME", home_value),
                ("XDG_DATA_HOME", data_home_value),
                ("XDG_DATA_DIRS", data_dirs_value),
            ] {
                command.env_remove(key);
                if let Some(value) = value {
                    command.env(key, value);
                }
            }
            let output = command.output().unwrap();
            if !output.status.success() {
                failures.push(format!(
                    "{label}:\n{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn desktop_lookup_distinguishes_native_names_from_replacement_characters() {
        use std::ffi::OsString;
        use std::fs;
        use std::os::unix::ffi::OsStringExt as _;
        use std::path::PathBuf;
        use std::process::Command;

        const CHILD_ROOT: &str = "CEFDETECTOR_TEST_DESKTOP_NATIVE_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            let content = fs::read_to_string(root.join("native.desktop")).unwrap();
            let (names, icon) = parse_desktop_entry(&content).unwrap();
            *super::DESKTOP_INDEX.lock().unwrap() =
                Some(names.into_iter().map(|name| (name, icon.clone())).collect());
            let config = crate::config::IconConfig {
                sources: vec![
                    crate::config::IconSource::Desktop,
                    crate::config::IconSource::Builtin,
                ],
                ..Default::default()
            };
            for name in [
                OsString::from_vec(b"APP-\xff".to_vec()),
                OsString::from_vec(b"APP-\xfe".to_vec()),
            ] {
                let path = root.join(name);
                fs::write(&path, []).unwrap();
                assert_eq!(super::find_icon_via_desktop_file(&path, &config), None);
                let RawIcon::PngOrIco(bytes) = super::get_app_icon(&path, &config) else {
                    panic!("unmatched native name did not continue to the builtin icon");
                };
                assert_eq!(&*bytes, include_bytes!("../icons/default_cef_icon.ico"));
            }
            for name in ["APP-�", "示例APP"] {
                let path = root.join(name);
                fs::write(&path, []).unwrap();
                assert_eq!(
                    super::find_icon_via_desktop_file(&path, &config),
                    Some(root.join("icon.png"))
                );
                let RawIcon::PngOrIco(bytes) = super::get_app_icon(&path, &config) else {
                    panic!("matching Unicode name lost its desktop icon");
                };
                assert_eq!(&*bytes, include_bytes!("../icons/32x32.png"));
            }
            return;
        }

        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let root = Fixture(
            std::env::temp_dir().join(format!("cefdetector-desktop-native-{}", std::process::id())),
        );
        fs::create_dir(&root.0).unwrap();
        let icon = root.0.join("icon.png");
        fs::write(&icon, include_bytes!("../icons/32x32.png")).unwrap();
        fs::write(
            root.0.join("native.desktop"),
            format!(
                "[Desktop Entry]\nExec=/opt/App-� %U\nTryExec=示例App\nIcon={}\n",
                icon.to_str().unwrap()
            ),
        )
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "icon_finder::tests::desktop_lookup_distinguishes_native_names_from_replacement_characters",
            ])
            .env(CHILD_ROOT, &root.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn squashfs_magic_search_handles_boundaries_and_returns_the_first_match() {
        for position in [
            0, 1, 8188, 8189, 8190, 8191, 8192, 8193, 16378, 16379, 16380, 16381,
        ] {
            let mut bytes = vec![b'x'; position];
            bytes.extend_from_slice(b"hsqs followed by another hsqs");
            assert_eq!(
                find_squashfs_offset(bytes.as_slice()),
                Some(position as u64)
            );
        }
        for bytes in [b"".as_slice(), b"h", b"hs", b"hsq", b"no hsx magic"] {
            assert_eq!(find_squashfs_offset(bytes), None);
        }
    }

    #[test]
    fn squashfs_magic_search_handles_short_and_interrupted_reads() {
        use std::io::{self, Read};

        struct ChunkedReader<'a> {
            bytes: &'a [u8],
            maximum: usize,
            reads: usize,
            error: io::ErrorKind,
        }

        impl Read for ChunkedReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                self.reads += 1;
                if self.reads == 2 {
                    return Err(self.error.into());
                }
                let count = buffer.len().min(self.maximum);
                self.bytes.read(&mut buffer[..count])
            }
        }

        for maximum in 1..=7 {
            let reader = ChunkedReader {
                bytes: b"___hsqs___hsqs",
                maximum,
                reads: 0,
                error: io::ErrorKind::Interrupted,
            };
            assert_eq!(find_squashfs_offset(reader), Some(3), "{maximum}");
        }
        let broken = ChunkedReader {
            bytes: b"___hsqs",
            maximum: 1,
            reads: 0,
            error: io::ErrorKind::PermissionDenied,
        };
        assert_eq!(find_squashfs_offset(broken), None);
    }

    #[test]
    fn desktop_exec_parser_handles_quotes_and_env() {
        assert_eq!(
            desktop_exec_tokens("env FOO=bar \"/opt/My App/app\" --flag %U"),
            ["env", "FOO=bar", "/opt/My App/app", "--flag", "%U"]
        );
        assert_eq!(
            executable_name("env FOO=bar \"/opt/My App/app\" --flag %U").as_deref(),
            Some(std::ffi::OsStr::new("app"))
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

    #[test]
    fn neighboring_icon_templates_and_builtin_source_are_configurable() {
        let root =
            std::env::temp_dir().join(format!("cefdetector-icon-config-{}", std::process::id()));
        let executable = root.join("bin/demo");
        let icon = root.join("bin/art/demo.mark");
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&executable, []).unwrap();
        std::fs::write(&icon, []).unwrap();

        let mut config = crate::config::IconConfig::default();
        config.neighbor.directories = vec!["art".into()];
        config.neighbor.names = vec!["{executable}.mark".into()];
        clear_icon_caches();
        assert_eq!(find_neighboring_icon(&executable, &config), Some(icon));

        config
            .sources
            .retain(|source| *source != crate::config::IconSource::Builtin);
        assert!(matches!(configured_fallback_icon(&config), RawIcon::Empty));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use std::fs;

    use super::{RawIcon, try_icon_from_path};

    #[test]
    fn icns_is_decoded_to_png_for_the_existing_gui_pipeline() {
        let mut image = icns::Image::new(icns::PixelFormat::RGBA, 32, 32);
        for pixel in image.data_mut().chunks_exact_mut(4) {
            pixel.copy_from_slice(&[20, 100, 220, 255]);
        }
        let mut family = icns::IconFamily::new();
        family.add_icon(&image).unwrap();
        let path =
            std::env::temp_dir().join(format!("cefdetector-icon-{}.icns", std::process::id()));
        family.write(fs::File::create(&path).unwrap()).unwrap();
        let icon =
            try_icon_from_path(&path, crate::config::IconConfig::default().max_file_bytes).unwrap();
        fs::remove_file(path).unwrap();
        let RawIcon::PngOrIco(bytes) = icon else {
            panic!("decoded ICNS did not use the raster icon path");
        };
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }
}
