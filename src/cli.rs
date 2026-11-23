use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::config::{self, AppConfig, CliConfig, LoadOptions, SortKey, SortOrder};
use crate::models::AppInfo;
use crate::search::core_search;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq)]
pub struct GuiOptions {
    pub config: AppConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    Toml,
    Json,
    Csv,
}

#[derive(Debug, PartialEq, Eq)]
struct CliOptions {
    output_format: OutputFormat,
    output_path: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    RootHelp,
    CliHelp,
    Version,
    RunCli(CliOptions),
    Config(ConfigCommand),
    #[cfg(feature = "gui")]
    LaunchGui,
}

#[derive(Debug, PartialEq, Eq)]
enum ConfigCommand {
    Help,
    Paths,
    Show,
    Validate(Option<PathBuf>),
}

fn print_root_help() {
    println!("CEF Detector {}", VERSION);
    println!();
    #[cfg(feature = "gui")]
    println!(
        "Usage: cefdetector [OPTIONS]\n       cefdetector [OPTIONS] cli [CLI OPTIONS]\n       cefdetector [OPTIONS] config <COMMAND>"
    );
    #[cfg(not(feature = "gui"))]
    println!("Usage: cefdetector [OPTIONS] <COMMAND>");
    println!();
    println!("Commands:");
    println!("  cli    Run the command-line scanner");
    println!("  config Inspect and validate configuration");
    println!();
    println!("Options:");
    println!("      --config <FILE>       Load an additional configuration file");
    println!("      --no-system-config    Do not load the system configuration");
    println!("      --no-user-config      Do not load the user configuration");
    println!("      --set <KEY=VALUE>     Override a configuration value using TOML syntax");
    println!();
    println!("  -h, --help         Print help information");
    println!("  -V, --version      Print version information");
}

fn print_cli_help() {
    println!("CEF Detector {}", VERSION);
    println!();
    println!("Usage: cefdetector cli [OPTIONS]");
    println!();
    println!("Options:");
    println!("  -h, --help       Print help information");
    println!("  -V, --version    Print version information");
    println!("  -T, --toml       Output results in TOML format");
    println!("  -J, --json       Output results in JSON format");
    println!("  -C, --csv        Output results in CSV format");
    println!("  -O, --output <FILE>  Write results to a file instead of stdout");
}

fn print_config_help() {
    println!("CEF Detector {}", VERSION);
    println!();
    println!("Usage: cefdetector [OPTIONS] config <COMMAND>");
    println!();
    println!("Commands:");
    println!("  paths            Print automatic configuration paths");
    println!("  show             Print the effective merged configuration");
    println!("  validate [FILE]  Validate the effective configuration or one file");
}

fn argument_text(argument: &OsStr) -> Result<&str, String> {
    argument.to_str().ok_or_else(|| {
        format!(
            "argument is not valid UTF-8: {}",
            argument.to_string_lossy()
        )
    })
}

fn parse_arguments(args: &[OsString]) -> Result<Action, String> {
    if args.first().is_some_and(|arg| arg == "cli") {
        return parse_cli_arguments(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "config") {
        return parse_config_arguments(&args[1..]);
    }

    parse_gui_arguments(args)
}

#[cfg(feature = "gui")]
fn parse_gui_arguments(args: &[OsString]) -> Result<Action, String> {
    if let Some(arg) = args.iter().find(|arg| {
        !(cfg!(target_os = "macos") && arg.to_str().is_some_and(|arg| arg.starts_with("-psn_")))
    }) {
        match argument_text(arg)? {
            "--help" | "-h" => return Ok(Action::RootHelp),
            "--version" | "-V" => return Ok(Action::Version),
            arg => return Err(format!("unknown option or command: {arg}")),
        }
    }

    Ok(Action::LaunchGui)
}

#[cfg(not(feature = "gui"))]
fn parse_gui_arguments(args: &[OsString]) -> Result<Action, String> {
    let Some(arg) = args.first() else {
        return Ok(Action::RootHelp);
    };

    match argument_text(arg)? {
        "--help" | "-h" => Ok(Action::RootHelp),
        "--version" | "-V" => Ok(Action::Version),
        arg => Err(format!("unknown option or command: {arg}")),
    }
}

fn parse_cli_arguments(args: &[OsString]) -> Result<Action, String> {
    if args.is_empty() {
        return Ok(Action::CliHelp);
    }

    let mut output_format = None;
    let mut output_path = None;
    let mut index = 0;

    while index < args.len() {
        match argument_text(&args[index])? {
            "--help" | "-h" => return Ok(Action::CliHelp),
            "--version" | "-V" => return Ok(Action::Version),
            "--toml" | "-T" => output_format = Some(OutputFormat::Toml),
            "--json" | "-J" => output_format = Some(OutputFormat::Json),
            "--csv" | "-C" => output_format = Some(OutputFormat::Csv),
            "--output" | "-O" => {
                let Some(path) = args.get(index + 1) else {
                    return Err("--output requires a file path".into());
                };
                output_path = Some(PathBuf::from(path));
                index += 1;
            }
            arg => return Err(format!("unknown CLI option: {arg}")),
        }
        index += 1;
    }

    match output_format {
        Some(output_format) => Ok(Action::RunCli(CliOptions {
            output_format,
            output_path,
        })),
        None => Ok(Action::CliHelp),
    }
}

fn parse_config_arguments(args: &[OsString]) -> Result<Action, String> {
    let command = match args {
        [] => ConfigCommand::Help,
        [value] if matches!(value.to_str(), Some("-h" | "--help")) => ConfigCommand::Help,
        [command] if command == "paths" => ConfigCommand::Paths,
        [command] if command == "show" => ConfigCommand::Show,
        [command] if command == "validate" => ConfigCommand::Validate(None),
        [command, path] if command == "validate" => {
            ConfigCommand::Validate(Some(PathBuf::from(path)))
        }
        [command, ..] => {
            return Err(format!(
                "unknown config command or arguments: {}",
                argument_text(command)?
            ));
        }
    };
    Ok(Action::Config(command))
}

fn extract_config_options(args: &[OsString]) -> Result<(Vec<OsString>, LoadOptions), String> {
    let mut command_args = Vec::new();
    let mut options = LoadOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--config") => {
                let Some(path) = args.get(index + 1) else {
                    return Err("--config requires a file path".into());
                };
                options.explicit_files.push(PathBuf::from(path));
                index += 1;
            }
            Some("--no-system-config") => options.no_system = true,
            Some("--no-user-config") => options.no_user = true,
            Some("--set") => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--set requires KEY=VALUE".into());
                };
                options.overrides.push(argument_text(value)?.to_owned());
                index += 1;
            }
            Some(arg) if arg.starts_with("--set=") => {
                let value = arg.trim_start_matches("--set=");
                if value.is_empty() {
                    return Err("--set requires KEY=VALUE".into());
                }
                options.overrides.push(value.to_owned());
            }
            _ => {
                if let Some(mut path) = args[index].as_encoded_bytes().strip_prefix(b"--config=") {
                    // Preserve the existing trim_start_matches behavior.
                    while let Some(rest) = path.strip_prefix(b"--config=") {
                        path = rest;
                    }
                    if path.is_empty() {
                        return Err("--config requires a file path".into());
                    }
                    // SAFETY: these bytes come from this OsStr in the same build.
                    // They were split only after a nonempty ASCII prefix.
                    let path = unsafe { OsStr::from_encoded_bytes_unchecked(path) };
                    options.explicit_files.push(PathBuf::from(path));
                } else {
                    command_args.push(args[index].clone());
                }
            }
        }
        index += 1;
    }
    Ok((command_args, options))
}

fn push_json_string(output: &mut String, value: &str) {
    output.push('"');
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{00}'..='\u{1f}' => {
                write!(output, "\\u{:04x}", ch as u32).unwrap();
            }
            _ => output.push(ch),
        }
    }
    output.push('"');
}

fn native_path_metadata(path: &Path, enabled: bool) -> Option<(&'static str, String)> {
    if !enabled || path.to_str().is_some() {
        return None;
    }

    let mut raw = String::new();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;

        for byte in path.as_os_str().as_bytes() {
            write!(raw, "{byte:02x}").unwrap();
        }
        Some(("unix-bytes-hex", raw))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;

        for unit in path.as_os_str().encode_wide() {
            write!(raw, "{unit:04x}").unwrap();
        }
        Some(("windows-utf16-hex", raw))
    }
}

fn format_json(results: &[AppInfo], config: &CliConfig) -> String {
    let pretty = config.pretty;
    let mut output = String::from("[");
    for (index, result) in results.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(if pretty {
            "\n  {\n    \"file\": "
        } else {
            "{\"file\":"
        });
        push_json_string(&mut output, &result.file.to_string_lossy());
        output.push_str(if pretty {
            ",\n    \"app_type\": "
        } else {
            ",\"app_type\":"
        });
        push_json_string(&mut output, &result.app_type);
        if pretty {
            write!(
                output,
                ",\n    \"size\": {},\n    \"is_running\": {},\n    \"is_dir\": {}",
                result.size, result.is_running, result.is_dir
            )
            .unwrap();
        } else {
            write!(
                output,
                ",\"size\":{},\"is_running\":{},\"is_dir\":{}",
                result.size, result.is_running, result.is_dir
            )
            .unwrap();
        }
        if let Some((encoding, raw)) = native_path_metadata(&result.file, config.native_paths) {
            output.push_str(if pretty {
                ",\n    \"file_raw_encoding\": "
            } else {
                ",\"file_raw_encoding\":"
            });
            push_json_string(&mut output, encoding);
            output.push_str(if pretty {
                ",\n    \"file_raw\": "
            } else {
                ",\"file_raw\":"
            });
            push_json_string(&mut output, &raw);
        }
        output.push_str(if pretty { "\n  }" } else { "}" });
    }
    if pretty && !results.is_empty() {
        output.push('\n');
    }
    output.push(']');
    output
}

fn push_toml_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\u{00}'..='\u{1f}' | '\u{7f}' => {
                write!(output, "\\u{:04X}", character as u32).unwrap();
            }
            _ => output.push(character),
        }
    }
    output.push('"');
}

fn push_csv_field(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        if character == '"' {
            output.push('"');
        }
        output.push(character);
    }
    output.push('"');
}

fn format_results(results: &[AppInfo], format: OutputFormat, config: &CliConfig) -> String {
    match format {
        OutputFormat::Json => format_json(results, config),
        OutputFormat::Toml => {
            let mut output = String::new();
            for result in results {
                let separator = if config.pretty { " = " } else { "=" };
                output.push_str("[[app]]\nfile");
                output.push_str(separator);
                push_toml_string(&mut output, &result.file.to_string_lossy());
                output.push_str("\napp_type");
                output.push_str(separator);
                push_toml_string(&mut output, &result.app_type);
                write!(
                    output,
                    "\nsize{separator}{}\nis_running{separator}{}\nis_dir{separator}{}",
                    result.size, result.is_running, result.is_dir
                )
                .unwrap();
                if let Some((encoding, raw)) =
                    native_path_metadata(&result.file, config.native_paths)
                {
                    output.push_str("\nfile_raw_encoding");
                    output.push_str(separator);
                    push_toml_string(&mut output, encoding);
                    output.push_str("\nfile_raw");
                    output.push_str(separator);
                    push_toml_string(&mut output, &raw);
                }
                output.push('\n');
                if config.pretty {
                    output.push('\n');
                }
            }
            output
        }
        OutputFormat::Csv => {
            let delimiter = config.csv.delimiter;
            let mut output = String::new();
            if config.csv.header {
                write!(
                    output,
                    "file{delimiter}app_type{delimiter}size{delimiter}is_running{delimiter}is_dir"
                )
                .unwrap();
                if config.native_paths {
                    for field in ["file_raw_encoding", "file_raw"] {
                        output.push(delimiter);
                        push_csv_field(&mut output, field);
                    }
                }
                output.push('\n');
            }
            for result in results {
                push_csv_field(&mut output, &result.file.to_string_lossy());
                output.push(delimiter);
                push_csv_field(&mut output, &result.app_type);
                write!(
                    output,
                    "{delimiter}{}{delimiter}{}{delimiter}{}",
                    result.size, result.is_running, result.is_dir
                )
                .unwrap();
                if config.native_paths {
                    output.push(delimiter);
                    if let Some((encoding, raw)) = native_path_metadata(&result.file, true) {
                        push_csv_field(&mut output, encoding);
                        output.push(delimiter);
                        push_csv_field(&mut output, &raw);
                    } else {
                        output.push(delimiter);
                    }
                }
                output.push('\n');
            }
            output
        }
    }
}

fn sort_results(results: &mut [AppInfo], key: SortKey, order: SortOrder) {
    results.sort_by(|left, right| {
        let ordering = match key {
            SortKey::Size => left.size.cmp(&right.size),
            SortKey::Name => std::path::Path::new(left.file.to_string_lossy().as_ref())
                .file_name()
                .cmp(&std::path::Path::new(right.file.to_string_lossy().as_ref()).file_name()),
            SortKey::Type => left.app_type.cmp(&right.app_type),
            SortKey::Path => left
                .file
                .to_string_lossy()
                .cmp(&right.file.to_string_lossy()),
            SortKey::Running => left.is_running.cmp(&right.is_running),
        };
        if order == SortOrder::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn run_cli(config: &AppConfig, options: CliOptions) -> Result<(), String> {
    let mut results = Vec::new();
    core_search(config, |info| results.push(info))
        .map_err(|error| format!("search failed: {error}"))?;
    sort_results(&mut results, config.cli.sort_by, config.cli.sort_order);

    let output = format_results(&results, options.output_format, &config.cli);
    if let Some(path) = options.output_path {
        use std::io::Write as _;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(config.cli.overwrite)
            .create_new(!config.cli.overwrite)
            .open(&path)
            .map_err(|error| format!("failed to open {} for output: {error}", path.display()))?;
        file.write_all(output.as_bytes())
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    } else {
        println!("{output}");
    }
    Ok(())
}

fn config_or_exit(result: Result<config::LoadedConfig, config::ConfigError>) -> AppConfig {
    result
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2);
        })
        .config
}

fn print_config_paths() {
    let paths = config::config_paths();
    println!(
        "system: {}",
        paths
            .system
            .as_deref()
            .map_or_else(|| "<unavailable>".into(), |path| path.display().to_string())
    );
    println!(
        "user: {}",
        paths
            .user
            .as_deref()
            .map_or_else(|| "<unavailable>".into(), |path| path.display().to_string())
    );
}

fn handle_config_command(command: ConfigCommand, options: &LoadOptions) {
    match command {
        ConfigCommand::Help => print_config_help(),
        ConfigCommand::Paths => print_config_paths(),
        ConfigCommand::Show => {
            let loaded = config::load(options).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2);
            });
            let output = config::format_effective(&loaded).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(1);
            });
            print!("{output}");
        }
        ConfigCommand::Validate(path) => {
            let validation_options = path.map_or_else(
                || options.clone(),
                |path| LoadOptions {
                    no_system: true,
                    no_user: true,
                    explicit_files: vec![path],
                    overrides: options.overrides.clone(),
                },
            );
            let loaded = config::load(&validation_options).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2);
            });
            println!(
                "Configuration is valid ({} file(s) loaded).",
                loaded.sources.len()
            );
        }
    }
}

pub fn handle_arguments() -> Option<GuiOptions> {
    let raw_args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (args, load_options) = extract_config_options(&raw_args).unwrap_or_else(|error| {
        eprintln!("Error: {error}");
        eprintln!("Run 'cefdetector --help' for usage information.");
        std::process::exit(2);
    });
    let help_command = if args.first().is_some_and(|arg| arg == "cli") {
        "cefdetector cli --help"
    } else if args.first().is_some_and(|arg| arg == "config") {
        "cefdetector config --help"
    } else {
        "cefdetector --help"
    };
    let action = parse_arguments(&args).unwrap_or_else(|error| {
        eprintln!("Error: {error}");
        eprintln!("Run '{help_command}' for usage information.");
        std::process::exit(2);
    });

    match action {
        Action::RootHelp => print_root_help(),
        Action::CliHelp => print_cli_help(),
        Action::Version => println!("cefdetector {VERSION}"),
        Action::RunCli(options) => {
            let config = config_or_exit(config::load_for_cli(&load_options));
            if let Err(error) = run_cli(&config, options) {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
        Action::Config(command) => handle_config_command(command, &load_options),
        #[cfg(feature = "gui")]
        Action::LaunchGui => {
            return Some(GuiOptions {
                config: config_or_exit(config::load(&load_options)),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{
        Action, CliOptions, ConfigCommand, OutputFormat, extract_config_options, format_json,
        format_results, native_path_metadata, parse_arguments, push_json_string,
    };
    use crate::config::CliConfig;
    use crate::models::AppInfo;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[cfg(unix)]
    fn non_unicode_argument() -> OsString {
        use std::os::unix::ffi::OsStringExt as _;

        OsString::from_vec(b"native-\xff.toml".to_vec())
    }

    #[cfg(windows)]
    fn non_unicode_argument() -> OsString {
        use std::os::windows::ffi::OsStringExt as _;

        OsString::from_wide(&[b'n' as u16, 0xD800, b'.' as u16, b't' as u16])
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn path_arguments_preserve_native_encoding() {
        let path = non_unicode_argument();
        assert!(path.to_str().is_none());
        let expected = PathBuf::from(&path);
        let mut inline = OsString::from("--config=");
        inline.push(&path);
        for input in [vec!["--config".into(), path.clone()], vec![inline]] {
            let (command, options) = extract_config_options(&input).unwrap();
            assert!(command.is_empty());
            assert_eq!(options.explicit_files, std::slice::from_ref(&expected));
        }
        assert_eq!(
            parse_arguments(&["config".into(), "validate".into(), path.clone()]),
            Ok(Action::Config(ConfigCommand::Validate(Some(
                expected.clone()
            ))))
        );
        assert_eq!(
            parse_arguments(&["cli".into(), "--json".into(), "--output".into(), path]),
            Ok(Action::RunCli(CliOptions {
                output_format: OutputFormat::Json,
                output_path: Some(expected),
            }))
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn non_unicode_options_and_overrides_return_argument_errors() {
        let invalid = non_unicode_argument();
        for input in [
            vec![invalid.clone()],
            vec!["cli".into(), invalid.clone()],
            vec!["config".into(), invalid.clone()],
        ] {
            assert!(
                parse_arguments(&input)
                    .unwrap_err()
                    .contains("not valid UTF-8")
            );
        }
        assert!(
            extract_config_options(&["--set".into(), invalid.clone()])
                .unwrap_err()
                .contains("not valid UTF-8")
        );
        let mut inline = OsString::from("--set=");
        inline.push(invalid);
        let (command, _) = extract_config_options(&[inline]).unwrap();
        assert!(
            parse_arguments(&command)
                .unwrap_err()
                .contains("not valid UTF-8")
        );
    }

    #[test]
    fn inline_config_paths_keep_existing_prefix_handling() {
        for (input, expected) in [
            ("--config=extra.toml", "extra.toml"),
            ("--config=--config=extra.toml", "extra.toml"),
            ("--config==extra.toml", "=extra.toml"),
            ("--config=目录/extra.toml", "目录/extra.toml"),
        ] {
            let (command, options) = extract_config_options(&args(&[input])).unwrap();
            assert!(command.is_empty());
            assert_eq!(options.explicit_files, [PathBuf::from(expected)]);
        }
        for input in ["--config=", "--config=--config="] {
            assert_eq!(
                extract_config_options(&args(&[input])).unwrap_err(),
                "--config requires a file path"
            );
        }
    }

    #[test]
    fn cli_subcommand_without_options_prints_cli_help() {
        assert_eq!(parse_arguments(&args(&["cli"])), Ok(Action::CliHelp));
    }

    #[test]
    fn cli_subcommand_owns_scanner_options() {
        assert_eq!(
            parse_arguments(&args(&["cli", "--json", "--output", "result.json"])),
            Ok(Action::RunCli(CliOptions {
                output_format: OutputFormat::Json,
                output_path: Some("result.json".into()),
            }))
        );
        assert_eq!(
            parse_arguments(&args(&["--json"])),
            Err("unknown option or command: --json".into())
        );
    }

    #[cfg(feature = "gui")]
    #[test]
    fn no_arguments_launches_the_gui() {
        assert_eq!(parse_arguments(&[]), Ok(Action::LaunchGui));
    }

    #[cfg(not(feature = "gui"))]
    #[test]
    fn no_arguments_prints_help_without_a_gui() {
        assert_eq!(parse_arguments(&[]), Ok(Action::RootHelp));
    }

    #[test]
    fn config_options_are_removed_without_mixing_subcommand_options() {
        let (command, options) = extract_config_options(&args(&[
            "--no-system-config",
            "cli",
            "--config",
            "extra.toml",
            "--json",
            "--set=search.walk_threads=2",
        ]))
        .unwrap();
        assert_eq!(command, args(&["cli", "--json"]));
        assert!(options.no_system);
        assert_eq!(
            options.explicit_files,
            [std::path::PathBuf::from("extra.toml")]
        );
        assert_eq!(options.overrides, ["search.walk_threads=2"]);
    }

    #[test]
    fn configuration_commands_are_separate_from_scanner_options() {
        assert_eq!(
            parse_arguments(&args(&["config", "paths"])),
            Ok(Action::Config(ConfigCommand::Paths))
        );
        assert!(parse_arguments(&args(&["cli", "paths"])).is_err());
    }

    #[test]
    fn json_strings_escape_control_characters() {
        let mut output = String::new();
        push_json_string(&mut output, "a\"b\\c\n\t\u{01}中文");
        assert_eq!(output, "\"a\\\"b\\\\c\\n\\t\\u0001中文\"");
    }

    #[test]
    fn json_output_keeps_the_cli_schema() {
        let output = format_json(
            &[AppInfo {
                file: "/tmp/app".into(),
                app_type: "CEF".into(),
                size: 42,
                is_running: true,
                is_dir: false,
            }],
            &CliConfig::default(),
        );
        assert_eq!(
            output,
            "[\n  {\n    \"file\": \"/tmp/app\",\n    \"app_type\": \"CEF\",\n    \"size\": 42,\n    \"is_running\": true,\n    \"is_dir\": false\n  }\n]"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn native_paths_keep_the_existing_text_output() {
        let results = [AppInfo {
            file: non_unicode_argument().into(),
            app_type: "CEF".into(),
            size: 42,
            is_running: false,
            is_dir: false,
        }];
        let display = results[0].file.to_string_lossy();
        let config = CliConfig::default();
        let json = format_results(&results, OutputFormat::Json, &config);
        assert!(json.contains(&format!("\"file\": \"{display}\"")));
        let output = format_results(&results, OutputFormat::Toml, &config);
        let toml: toml::Value = toml::from_str(&output).unwrap();
        assert_eq!(toml["app"][0]["file"].as_str(), Some(display.as_ref()));
        let csv = format_results(&results, OutputFormat::Csv, &config);
        assert!(csv.contains(&format!("\"{display}\",\"CEF\",42,false,false\n")));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn raw_path_metadata_round_trips_native_units() {
        #[cfg(unix)]
        let (path, expected_encoding, expected_raw) = {
            use std::os::unix::ffi::OsStringExt as _;

            (
                PathBuf::from(OsString::from_vec(vec![
                    0, 1, 0x7f, 0x80, 0xc0, 0xe2, 0xff, b'/', b'"', b'\n', b'a',
                ])),
                "unix-bytes-hex",
                "00017f80c0e2ff2f220a61",
            )
        };
        #[cfg(windows)]
        let (path, expected_encoding, expected_raw) = {
            use std::os::windows::ffi::OsStringExt as _;

            (
                PathBuf::from(OsString::from_wide(&[
                    0, 1, 0x7f, 0x80, 0x1234, 0xd800, 0xdc00, 0xdc01, 0x61, 0xffff,
                ])),
                "windows-utf16-hex",
                "00000001007f00801234d800dc00dc010061ffff",
            )
        };
        assert!(path.to_str().is_none());
        assert_eq!(native_path_metadata(&path, false), None);
        let (encoding, raw) = native_path_metadata(&path, true).unwrap();
        assert_eq!(encoding, expected_encoding);
        assert_eq!(raw, expected_raw);

        #[cfg(unix)]
        let restored = {
            use std::os::unix::ffi::OsStringExt as _;

            OsString::from_vec(
                raw.as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect(),
            )
        };
        #[cfg(windows)]
        let restored = {
            use std::os::windows::ffi::OsStringExt as _;

            let units: Vec<_> = raw
                .as_bytes()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|unit| u16::from_str_radix(std::str::from_utf8(unit).unwrap(), 16).unwrap())
                .collect();
            OsString::from_wide(&units)
        };
        assert_eq!(restored, path.as_os_str());
    }

    #[test]
    fn unicode_paths_do_not_add_raw_metadata() {
        for file in ["", "/ASCII/app", "C:\\中文\\😀�,\"\n\t.exe"] {
            let results = [AppInfo {
                file: file.into(),
                app_type: "CEF".into(),
                size: 42,
                is_running: false,
                is_dir: false,
            }];
            assert_eq!(native_path_metadata(&results[0].file, true), None);
            for pretty in [false, true] {
                let mut config = CliConfig {
                    pretty,
                    ..Default::default()
                };
                for format in [OutputFormat::Json, OutputFormat::Toml] {
                    let before = format_results(&results, format, &config);
                    config.native_paths = true;
                    assert_eq!(format_results(&results, format, &config), before);
                    config.native_paths = false;
                }
            }
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn native_path_output_preserves_text_and_adds_metadata() {
        let native = PathBuf::from(non_unicode_argument());
        let display = native.to_string_lossy().into_owned();
        let (encoding, raw) = native_path_metadata(&native, true).unwrap();
        let results = [native, PathBuf::from(&display)].map(|file| AppInfo {
            file,
            app_type: "CEF".into(),
            size: 42,
            is_running: false,
            is_dir: false,
        });
        for pretty in [false, true] {
            let mut config = CliConfig {
                pretty,
                native_paths: true,
                ..Default::default()
            };
            let json = format_results(&results, OutputFormat::Json, &config);
            let separator = if pretty { ": " } else { ":" };
            assert_eq!(json.matches("\"file_raw_encoding\"").count(), 1);
            assert_eq!(json.matches("\"file_raw\"").count(), 1);
            assert!(json.contains(&format!("\"file_raw_encoding\"{separator}\"{encoding}\"")));
            assert!(json.contains(&format!("\"file_raw\"{separator}\"{raw}\"")));

            let output = format_results(&results, OutputFormat::Toml, &config);
            let toml: toml::Value = toml::from_str(&output).unwrap();
            assert_eq!(toml["app"][0]["file"].as_str(), Some(display.as_str()));
            assert_eq!(toml["app"][0]["file_raw_encoding"].as_str(), Some(encoding));
            assert_eq!(toml["app"][0]["file_raw"].as_str(), Some(raw.as_str()));
            assert_eq!(toml["app"][0].as_table().unwrap().len(), 7);
            assert_eq!(toml["app"][1]["file"].as_str(), Some(display.as_str()));
            assert_eq!(toml["app"][1].as_table().unwrap().len(), 5);

            for delimiter in [',', ';', '\t', '-', '中', 'c', 'w', 'o'] {
                config.csv.delimiter = delimiter;
                for header in [false, true] {
                    config.csv.header = header;
                    let expected_header = if header {
                        format!(
                            "file{delimiter}app_type{delimiter}size{delimiter}is_running{delimiter}is_dir{delimiter}\"file_raw_encoding\"{delimiter}\"file_raw\"\n"
                        )
                    } else {
                        String::new()
                    };
                    let mut expected = expected_header.clone();
                    expected.push_str(&format!(
                        "\"{display}\"{delimiter}\"CEF\"{delimiter}42{delimiter}false{delimiter}false{delimiter}\"{encoding}\"{delimiter}\"{raw}\"\n\"{display}\"{delimiter}\"CEF\"{delimiter}42{delimiter}false{delimiter}false{delimiter}{delimiter}\n"
                    ));
                    assert_eq!(
                        format_results(&results, OutputFormat::Csv, &config),
                        expected
                    );
                    assert_eq!(
                        format_results(&[], OutputFormat::Csv, &config),
                        expected_header
                    );
                }
            }
            assert_eq!(format_results(&[], OutputFormat::Json, &config), "[]");
            assert_eq!(format_results(&[], OutputFormat::Toml, &config), "");
        }
    }

    #[test]
    fn path_sorting_keeps_text_order_instead_of_component_order() {
        let mut results: Vec<_> = ["/a/z", "/a-z"]
            .map(|file| AppInfo {
                file: file.into(),
                app_type: "CEF".into(),
                size: 0,
                is_running: false,
                is_dir: false,
            })
            .into();
        super::sort_results(
            &mut results,
            crate::config::SortKey::Path,
            crate::config::SortOrder::Ascending,
        );
        assert_eq!(results[0].file.as_os_str(), "/a-z");
        assert_eq!(results[1].file.as_os_str(), "/a/z");
    }

    #[test]
    fn compact_json_and_csv_settings_are_applied() {
        let results = [AppInfo {
            file: "/tmp/a,\"b\n".into(),
            app_type: "CEF".into(),
            size: 42,
            is_running: true,
            is_dir: false,
        }];
        assert_eq!(
            format_json(
                &results,
                &CliConfig {
                    pretty: false,
                    ..Default::default()
                }
            ),
            "[{\"file\":\"/tmp/a,\\\"b\\n\",\"app_type\":\"CEF\",\"size\":42,\"is_running\":true,\"is_dir\":false}]"
        );

        let mut config = CliConfig::default();
        config.csv.header = false;
        config.csv.delimiter = ';';
        assert_eq!(
            format_results(&results, OutputFormat::Csv, &config),
            "\"/tmp/a,\"\"b\n\";\"CEF\";42;true;false\n"
        );
    }

    #[test]
    fn toml_output_escapes_control_characters() {
        let output = format_results(
            &[AppInfo {
                file: "C:\\line\nnext".into(),
                app_type: "C\"EF".into(),
                size: 1,
                is_running: false,
                is_dir: false,
            }],
            OutputFormat::Toml,
            &CliConfig::default(),
        );
        let value: toml::Value = toml::from_str(&output).unwrap();
        assert_eq!(value["app"][0]["file"].as_str(), Some("C:\\line\nnext"));
        assert_eq!(value["app"][0]["app_type"].as_str(), Some("C\"EF"));
    }
}
