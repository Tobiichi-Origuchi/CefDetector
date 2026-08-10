#compdef cefdetector

_cefdetector() {
    local command word
    for word in "${words[@]:1}"; do
        if [[ "${word}" == "cli" || "${word}" == "config" ]]; then
            command="${word}"
            break
        fi
    done

    local -a global_options
    global_options=(
        '--config[Load an additional configuration file]:file:_files'
        '--no-system-config[Do not load the system configuration]'
        '--no-user-config[Do not load the user configuration]'
        '--set[Override a configuration value using TOML syntax]:key=value:'
    )

    if [[ "${command}" == "cli" ]]; then
        _arguments \
            "${global_options[@]}" \
            '(-h --help)'{-h,--help}'[Print help information]' \
            '(-V --version)'{-V,--version}'[Print version information]' \
            '(-T --toml)'{-T,--toml}'[Output results in TOML format]' \
            '(-J --json)'{-J,--json}'[Output results in JSON format]' \
            '(-C --csv)'{-C,--csv}'[Output results in CSV format]' \
            '(-O --output)'{-O,--output}'[Output results to the specified file path instead of stdout]:file:_files'
        return
    fi

    if [[ "${command}" == "config" ]]; then
        _arguments \
            "${global_options[@]}" \
            '(-h --help)'{-h,--help}'[Print help information]' \
            '1:configuration command:((paths\:"Print automatic configuration paths" show\:"Print the effective merged configuration" validate\:"Validate configuration"))' \
            '2:configuration file:_files'
        return
    fi

    _arguments \
        "${global_options[@]}" \
        '(-h --help)'{-h,--help}'[Print help information]' \
        '(-V --version)'{-V,--version}'[Print version information]' \
        '--system-font[Use platform system fonts instead of embedded fonts]' \
        '1:command:((cli\:"Run the command-line scanner" config\:"Inspect and validate configuration"))'
}

_cefdetector "$@"
