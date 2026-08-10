set -l cefdetector_no_command "not __fish_seen_subcommand_from cli config"

complete -c cefdetector -n "$cefdetector_no_command" -f -a cli -d "Run the command-line scanner"
complete -c cefdetector -n "$cefdetector_no_command" -f -a config -d "Inspect and validate configuration"
complete -c cefdetector -n "$cefdetector_no_command" -s h -l help -d "Print help information"
complete -c cefdetector -n "$cefdetector_no_command" -s V -l version -d "Print version information"
complete -c cefdetector -n "$cefdetector_no_command" -l system-font -d "Use platform system fonts instead of embedded fonts"

complete -c cefdetector -l config -r -F -d "Load an additional configuration file"
complete -c cefdetector -l no-system-config -d "Do not load the system configuration"
complete -c cefdetector -l no-user-config -d "Do not load the user configuration"
complete -c cefdetector -l set -r -d "Override a configuration value using TOML syntax"

complete -c cefdetector -n "__fish_seen_subcommand_from cli" -s h -l help -d "Print help information"
complete -c cefdetector -n "__fish_seen_subcommand_from cli" -s V -l version -d "Print version information"
complete -c cefdetector -n "__fish_seen_subcommand_from cli" -s T -l toml -d "Output results in TOML format"
complete -c cefdetector -n "__fish_seen_subcommand_from cli" -s J -l json -d "Output results in JSON format"
complete -c cefdetector -n "__fish_seen_subcommand_from cli" -s C -l csv -d "Output results in CSV format"
complete -c cefdetector -n "__fish_seen_subcommand_from cli" -s O -l output -r -F -d "Output results to the specified file path instead of stdout"

complete -c cefdetector -n "__fish_seen_subcommand_from config; and not __fish_seen_subcommand_from paths show validate" -f -a paths -d "Print automatic configuration paths"
complete -c cefdetector -n "__fish_seen_subcommand_from config; and not __fish_seen_subcommand_from paths show validate" -f -a show -d "Print the effective merged configuration"
complete -c cefdetector -n "__fish_seen_subcommand_from config; and not __fish_seen_subcommand_from paths show validate" -f -a validate -d "Validate configuration"
complete -c cefdetector -n "__fish_seen_subcommand_from config; and __fish_seen_subcommand_from validate" -F
