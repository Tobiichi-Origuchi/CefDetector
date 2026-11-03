use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::collections::HashSet;
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
static DESKTOP_INDEX: LazyLock<Mutex<Option<HashMap<String, String>>>> =
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

pub fn find_neighboring_icon(exe_path: &Path, config: &IconConfig) -> Option<PathBuf> {
    let parent = exe_path.parent()?;

    if let Some(cached) = NEIGHBOR_ICON_CACHE.lock().unwrap().get(parent) {
        return cached.clone();
    }

    let exe_name = exe_path.file_name()?.to_string_lossy().to_string();

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
                let name = template.replace("{executable}", &exe_name);
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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
pub fn find_icon_via_appimage(exe_path: &Path) -> Option<RawIcon> {
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

pub fn get_app_icon(path: String, config: &IconConfig) -> RawIcon {
    if !config.enabled {
        return RawIcon::Empty;
    }
    let exe_path = Path::new(&path);

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
            IconSource::Pe => find_icon_via_pe(exe_path),
            IconSource::Appimage => {
                #[cfg(target_os = "linux")]
                {
                    find_icon_via_appimage(exe_path)
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
                        .then(|| crate::package_manager::find_icon_via_package_manager(exe_path))
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
                assert!(find_icon_via_appimage(&path).is_none(), "{name}");
                let RawIcon::PngOrIco(bytes) =
                    get_app_icon(path.to_string_lossy().into_owned(), &config)
                else {
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
    #[test]
    fn appimage_icon_lookup_preserves_regular_files_and_symlinks() {
        use backhand::{FilesystemWriter, NodeHeader};
        use std::io::Cursor;
        use std::os::unix::fs::symlink;

        let root = TempDirectory::new("appimage-regular-files");
        let expected = include_bytes!("../icons/default_cef_icon.ico");
        let mut writer = FilesystemWriter::default();
        writer
            .push_file(Cursor::new(expected), "/icon.ico", NodeHeader::default())
            .unwrap();
        writer
            .push_symlink("icon.ico", "/.DirIcon", NodeHeader::default())
            .unwrap();
        let path = root.0.join("regular.AppImage");
        writer.write(fs::File::create(&path).unwrap()).unwrap();
        let link = root.0.join("link.appimage");
        symlink(&path, &link).unwrap();
        for path in [&path, &link] {
            let Some(RawIcon::PngOrIco(bytes)) = find_icon_via_appimage(path) else {
                panic!("{} did not produce an icon", path.display());
            };
            assert_eq!(&*bytes, expected);
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
        let actual = get_app_icon(app.to_string_lossy().into_owned(), &config);
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
        assert!(matches!(
            get_app_icon(app.to_string_lossy().into_owned(), &config),
            RawIcon::Empty
        ));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{
        RawIcon, clear_icon_caches, configured_fallback_icon, desktop_exec_tokens, executable_name,
        find_neighboring_icon, find_squashfs_offset, parse_desktop_entry,
    };

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
