use std::fmt::Write as _;

use crate::models::AppInfo;
use crate::search::core_search;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GuiOptions {
    #[cfg(feature = "gui")]
    pub system_font: bool,
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
    #[cfg(feature = "gui")]
    LaunchGui(GuiOptions),
}

fn print_root_help() {
    println!("CEF Detector {}", VERSION);
    println!();
    #[cfg(feature = "gui")]
    println!("Usage: cefdetector [GUI OPTIONS]\n       cefdetector cli [CLI OPTIONS]");
    #[cfg(not(feature = "gui"))]
    println!("Usage: cefdetector <COMMAND>");
    println!();
    println!("Commands:");
    println!("  cli    Run the command-line scanner");
    println!();
    #[cfg(feature = "gui")]
    println!("GUI options:");
    #[cfg(not(feature = "gui"))]
    println!("Options:");
    println!("  -h, --help         Print help information");
    println!("  -V, --version      Print version information");
    #[cfg(feature = "gui")]
    println!("      --system-font  Use platform system fonts instead of embedded fonts");
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

fn parse_arguments(args: &[String]) -> Result<Action, String> {
    if args.first().is_some_and(|arg| arg == "cli") {
        return parse_cli_arguments(&args[1..]);
    }

    parse_gui_arguments(args)
}

#[cfg(feature = "gui")]
fn parse_gui_arguments(args: &[String]) -> Result<Action, String> {
    let mut options = GuiOptions::default();

    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Action::RootHelp),
            "--version" | "-V" => return Ok(Action::Version),
            "--system-font" => options.system_font = true,
            #[cfg(target_os = "macos")]
            arg if arg.starts_with("-psn_") => {
                // Finder may append a process serial number when launching an app bundle.
            }
            _ => return Err(format!("unknown GUI option: {arg}")),
        }
    }

    Ok(Action::LaunchGui(options))
}

#[cfg(not(feature = "gui"))]
fn parse_gui_arguments(args: &[String]) -> Result<Action, String> {
    let Some(arg) = args.first() else {
        return Ok(Action::RootHelp);
    };

    match arg.as_str() {
        "--help" | "-h" => Ok(Action::RootHelp),
        "--version" | "-V" => Ok(Action::Version),
        _ => Err(format!("unknown GUI option: {arg}")),
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

fn format_json(results: &[AppInfo]) -> String {
    let mut output = String::from("[");
    for (index, result) in results.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("\n  {\n    \"file\": ");
        push_json_string(&mut output, &result.file);
        output.push_str(",\n    \"app_type\": ");
        push_json_string(&mut output, &result.app_type);
        write!(
            output,
            ",\n    \"size\": {},\n    \"is_running\": {},\n    \"is_dir\": {}\n  }}",
            result.size, result.is_running, result.is_dir
        )
        .unwrap();
    }
    if !results.is_empty() {
        output.push('\n');
    }
    output.push(']');
    output
}

fn format_results(results: &[AppInfo], format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => format_json(results),
        OutputFormat::Toml => {
            let mut output = String::new();
            for result in results {
                output.push_str("[[app]]\n");
                output.push_str(&format!(
                    "file = \"{}\"\n",
                    result.file.replace("\\", "\\\\").replace("\"", "\\\"")
                ));
                output.push_str(&format!("app_type = \"{}\"\n", result.app_type));
                output.push_str(&format!("size = {}\n", result.size));
                output.push_str(&format!("is_running = {}\n", result.is_running));
                output.push_str(&format!("is_dir = {}\n\n", result.is_dir));
            }
            output
        }
        OutputFormat::Csv => {
            let mut output = String::from("file,app_type,size,is_running,is_dir\n");
            for result in results {
                let escaped_file = result.file.replace('"', "\"\"");
                output.push_str(&format!(
                    "\"{}\",\"{}\",{},{},{}\n",
                    escaped_file, result.app_type, result.size, result.is_running, result.is_dir
                ));
            }
            output
        }
    }
}

fn run_cli(options: CliOptions) -> Result<(), String> {
    let mut results = Vec::new();
    core_search(|info| results.push(info)).map_err(|error| format!("search failed: {error}"))?;

    let output = format_results(&results, options.output_format);
    if let Some(path) = options.output_path {
        std::fs::write(&path, output)
            .map_err(|error| format!("failed to write {path}: {error}"))?;
    } else {
        println!("{output}");
    }
    Ok(())
}

pub fn handle_arguments() -> Option<GuiOptions> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let help_command = if args.first().is_some_and(|arg| arg == "cli") {
        "cefdetector cli --help"
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
            if let Err(error) = run_cli(options) {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
        #[cfg(feature = "gui")]
        Action::LaunchGui(options) => return Some(options),
    }

    None
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "gui")]
    use super::GuiOptions;
    use super::{Action, CliOptions, OutputFormat, format_json, parse_arguments, push_json_string};
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
            Err("unknown GUI option: --json".into())
        );
    }

    #[cfg(feature = "gui")]
    #[test]
    fn root_arguments_only_configure_the_gui() {
        assert_eq!(
            parse_arguments(&args(&["--system-font"])),
            Ok(Action::LaunchGui(GuiOptions { system_font: true }))
        );
        assert_eq!(
            parse_arguments(&args(&["cli", "--system-font"])),
            Err("unknown CLI option: --system-font".into())
        );
    }

    #[cfg(feature = "gui")]
    #[test]
    fn no_arguments_launches_the_gui() {
        assert_eq!(
            parse_arguments(&[]),
            Ok(Action::LaunchGui(GuiOptions::default()))
        );
    }

    #[cfg(not(feature = "gui"))]
    #[test]
    fn no_arguments_prints_help_without_a_gui() {
        assert_eq!(parse_arguments(&[]), Ok(Action::RootHelp));
    }

    #[test]
    fn json_strings_escape_control_characters() {
        let mut output = String::new();
        push_json_string(&mut output, "a\"b\\c\n\t\u{01}中文");
        assert_eq!(output, "\"a\\\"b\\\\c\\n\\t\\u0001中文\"");
    }

    #[test]
    fn json_output_keeps_the_cli_schema() {
        let output = format_json(&[AppInfo {
            file: "/tmp/app".into(),
            app_type: "CEF".into(),
            size: 42,
            is_running: true,
            is_dir: false,
        }]);
        assert_eq!(
            output,
            "[\n  {\n    \"file\": \"/tmp/app\",\n    \"app_type\": \"CEF\",\n    \"size\": 42,\n    \"is_running\": true,\n    \"is_dir\": false\n  }\n]"
        );
    }
}
