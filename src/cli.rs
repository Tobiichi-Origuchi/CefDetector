use std::fmt::Write as _;

use crate::models::AppInfo;
use crate::search::core_search;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, PartialEq)]
enum OutputFormat {
    Toml,
    Json,
    Csv,
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

pub fn handle_cli() {
    let args: Vec<String> = std::env::args().collect();
    let mut show_help = false;
    let mut show_version = false;
    let mut output_format: Option<OutputFormat> = None;
    let mut output_path = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => show_help = true,
            "--version" | "-V" => show_version = true,
            "--toml" | "-T" => output_format = Some(OutputFormat::Toml),
            "--json" | "-J" => output_format = Some(OutputFormat::Json),
            "--csv" | "-C" => output_format = Some(OutputFormat::Csv),
            "--output" | "-O" => {
                if i + 1 < args.len() {
                    output_path = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --output requires a file path");
                    std::process::exit(1);
                }
            }
            _ => {
                // Ignore other args that might be passed by OS/Tauri
            }
        }
        i += 1;
    }

    if show_version {
        println!("cefdetector {}", VERSION);
        std::process::exit(0);
    }

    if show_help {
        println!("CEF Detector {}", VERSION);
        println!();
        println!("Usage: cefdetector [OPTIONS]");
        println!();
        println!("Options:");
        println!("  -h, --help       Print help information");
        println!("  -V, --version    Print version information");
        println!("  -T, --toml       Output results in TOML format");
        println!("  -J, --json       Output results in JSON format");
        println!("  -C, --csv        Output results in CSV format");
        println!("  -O, --output     Output results to the specified file path instead of stdout");
        std::process::exit(0);
    }

    if let Some(fmt) = output_format {
        let mut results = Vec::new();
        if let Err(error) = core_search(|info| {
            results.push(info);
        }) {
            eprintln!("Search failed: {error}");
            std::process::exit(1);
        }

        let output_str = match fmt {
            OutputFormat::Json => format_json(&results),
            OutputFormat::Toml => {
                let mut s = String::new();
                for r in &results {
                    s.push_str("[[app]]\n");
                    s.push_str(&format!(
                        "file = \"{}\"\n",
                        r.file.replace("\\", "\\\\").replace("\"", "\\\"")
                    ));
                    s.push_str(&format!("app_type = \"{}\"\n", r.app_type));
                    s.push_str(&format!("size = {}\n", r.size));
                    s.push_str(&format!("is_running = {}\n", r.is_running));
                    s.push_str(&format!("is_dir = {}\n\n", r.is_dir));
                }
                s
            }
            OutputFormat::Csv => {
                let mut s = String::from("file,app_type,size,is_running,is_dir\n");
                for r in &results {
                    let escaped_file = r.file.replace("\"", "\"\"");
                    s.push_str(&format!(
                        "\"{}\",\"{}\",{},{},{}\n",
                        escaped_file, r.app_type, r.size, r.is_running, r.is_dir
                    ));
                }
                s
            }
        };

        if let Some(path) = output_path {
            if let Err(e) = std::fs::write(&path, output_str) {
                eprintln!("Error writing to {}: {}", path, e);
                std::process::exit(1);
            }
        } else {
            println!("{}", output_str);
        }
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::{format_json, push_json_string};
    use crate::models::AppInfo;

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
