use std::fmt::Write as _;
use std::path::PathBuf;

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
    output_path: Option<String>,
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

fn parse_arguments(args: &[String]) -> Result<Action, String> {
    if args.first().is_some_and(|arg| arg == "cli") {
        return parse_cli_arguments(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "config") {
        return parse_config_arguments(&args[1..]);
    }

    parse_gui_arguments(args)
}

#[cfg(feature = "gui")]
fn parse_gui_arguments(args: &[String]) -> Result<Action, String> {
    if let Some(arg) = args
        .iter()
        .find(|arg| !(cfg!(target_os = "macos") && arg.starts_with("-psn_")))
    {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Action::RootHelp),
            "--version" | "-V" => return Ok(Action::Version),
            _ => return Err(format!("unknown option or command: {arg}")),
        }
    }

    Ok(Action::LaunchGui)
}

#[cfg(not(feature = "gui"))]
fn parse_gui_arguments(args: &[String]) -> Result<Action, String> {
    let Some(arg) = args.first() else {
        return Ok(Action::RootHelp);
    };

    match arg.as_str() {
        "--help" | "-h" => Ok(Action::RootHelp),
        "--version" | "-V" => Ok(Action::Version),
        _ => Err(format!("unknown option or command: {arg}")),
    }
}

fn parse_cli_arguments(args: &[String]) -> Result<Action, String> {
    if args.is_empty() {
        return Ok(Action::CliHelp);
    }

    let mut output_format = None;
    let mut output_path = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => return Ok(Action::CliHelp),
            "--version" | "-V" => return Ok(Action::Version),
            "--toml" | "-T" => output_format = Some(OutputFormat::Toml),
            "--json" | "-J" => output_format = Some(OutputFormat::Json),
            "--csv" | "-C" => output_format = Some(OutputFormat::Csv),
            "--output" | "-O" => {
                let Some(path) = args.get(index + 1) else {
                    return Err("--output requires a file path".into());
                };
                output_path = Some(path.clone());
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

fn parse_config_arguments(args: &[String]) -> Result<Action, String> {
    let command = match args {
        [] => ConfigCommand::Help,
        [value] if matches!(value.as_str(), "-h" | "--help") => ConfigCommand::Help,
        [command] if command == "paths" => ConfigCommand::Paths,
        [command] if command == "show" => ConfigCommand::Show,
        [command] if command == "validate" => ConfigCommand::Validate(None),
        [command, path] if command == "validate" => {
            ConfigCommand::Validate(Some(PathBuf::from(path)))
        }
        [command, ..] => return Err(format!("unknown config command or arguments: {command}")),
    };
    Ok(Action::Config(command))
}

fn extract_config_options(args: &[String]) -> Result<(Vec<String>, LoadOptions), String> {
    let mut command_args = Vec::new();
    let mut options = LoadOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                let Some(path) = args.get(index + 1) else {
                    return Err("--config requires a file path".into());
                };
                options.explicit_files.push(PathBuf::from(path));
                index += 1;
            }
            "--no-system-config" => options.no_system = true,
            "--no-user-config" => options.no_user = true,
            "--set" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("--set requires KEY=VALUE".into());
                };
                options.overrides.push(value.clone());
                index += 1;
            }
            arg if arg.starts_with("--config=") => {
                let path = arg.trim_start_matches("--config=");
                if path.is_empty() {
                    return Err("--config requires a file path".into());
                }
                options.explicit_files.push(PathBuf::from(path));
            }
            arg if arg.starts_with("--set=") => {
                let value = arg.trim_start_matches("--set=");
                if value.is_empty() {
                    return Err("--set requires KEY=VALUE".into());
                }
                options.overrides.push(value.to_owned());
            }
            _ => command_args.push(args[index].clone()),
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

fn format_json(results: &[AppInfo], pretty: bool) -> String {
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
        push_json_string(&mut output, &result.file);
        output.push_str(if pretty {
            ",\n    \"app_type\": "
        } else {
            ",\"app_type\":"
        });
        push_json_string(&mut output, &result.app_type);
        if pretty {
            write!(
                output,
                ",\n    \"size\": {},\n    \"is_running\": {},\n    \"is_dir\": {}\n  }}",
                result.size, result.is_running, result.is_dir
            )
            .unwrap();
        } else {
            write!(
                output,
                ",\"size\":{},\"is_running\":{},\"is_dir\":{}}}",
                result.size, result.is_running, result.is_dir
            )
            .unwrap();
        }
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
        OutputFormat::Json => format_json(results, config.pretty),
        OutputFormat::Toml => {
            let mut output = String::new();
            for result in results {
                let separator = if config.pretty { " = " } else { "=" };
                output.push_str("[[app]]\nfile");
                output.push_str(separator);
                push_toml_string(&mut output, &result.file);
                output.push_str("\napp_type");
                output.push_str(separator);
                push_toml_string(&mut output, &result.app_type);
                write!(
                    output,
                    "\nsize{separator}{}\nis_running{separator}{}\nis_dir{separator}{}\n",
                    result.size, result.is_running, result.is_dir
                )
                .unwrap();
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
                writeln!(
                    output,
                    "file{delimiter}app_type{delimiter}size{delimiter}is_running{delimiter}is_dir"
                )
                .unwrap();
            }
            for result in results {
                push_csv_field(&mut output, &result.file);
                output.push(delimiter);
                push_csv_field(&mut output, &result.app_type);
                writeln!(
                    output,
                    "{delimiter}{}{delimiter}{}{delimiter}{}",
                    result.size, result.is_running, result.is_dir
                )
                .unwrap();
            }
            output
        }
    }
}

fn sort_results(results: &mut [AppInfo], key: SortKey, order: SortOrder) {
    results.sort_by(|left, right| {
        let ordering = match key {
            SortKey::Size => left.size.cmp(&right.size),
            SortKey::Name => std::path::Path::new(&left.file)
                .file_name()
                .cmp(&std::path::Path::new(&right.file).file_name()),
            SortKey::Type => left.app_type.cmp(&right.app_type),
            SortKey::Path => left.file.cmp(&right.file),
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
            .map_err(|error| format!("failed to open {path} for output: {error}"))?;
        file.write_all(output.as_bytes())
            .map_err(|error| format!("failed to write {path}: {error}"))?;
    } else {
        println!("{output}");
    }
    Ok(())
}

fn load_config(options: &LoadOptions) -> AppConfig {
    config::load(options)
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
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
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
            let config = load_config(&load_options);
            if let Err(error) = run_cli(&config, options) {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
        Action::Config(command) => handle_config_command(command, &load_options),
        #[cfg(feature = "gui")]
        Action::LaunchGui => {
            return Some(GuiOptions {
                config: load_config(&load_options),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        Action, CliOptions, ConfigCommand, OutputFormat, extract_config_options, format_json,
        format_results, parse_arguments, push_json_string,
    };
    use crate::config::CliConfig;
    use crate::models::AppInfo;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
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
            true,
        );
        assert_eq!(
            output,
            "[\n  {\n    \"file\": \"/tmp/app\",\n    \"app_type\": \"CEF\",\n    \"size\": 42,\n    \"is_running\": true,\n    \"is_dir\": false\n  }\n]"
        );
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
            format_json(&results, false),
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
