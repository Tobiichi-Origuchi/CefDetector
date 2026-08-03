#compdef cefdetector

_cefdetector() {
    if (( CURRENT > 2 )) && [[ "${words[2]}" == "cli" ]]; then
        _arguments \
            '(-h --help)'{-h,--help}'[Print help information]' \
            '(-V --version)'{-V,--version}'[Print version information]' \
            '(-T --toml)'{-T,--toml}'[Output results in TOML format]' \
            '(-J --json)'{-J,--json}'[Output results in JSON format]' \
            '(-C --csv)'{-C,--csv}'[Output results in CSV format]' \
            '(-O --output)'{-O,--output}'[Output results to the specified file path instead of stdout]:file:_files'
        return
    fi

    _arguments \
        '(-h --help)'{-h,--help}'[Print help information]' \
        '(-V --version)'{-V,--version}'[Print version information]' \
        '--system-font[Use platform system fonts instead of embedded fonts]' \
        '1:command:((cli\:"Run the command-line scanner"))'
}

_cefdetector "$@"
