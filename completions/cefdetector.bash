_cefdetector() {
    local cur prev opts
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    if [[ "${COMP_WORDS[1]:-}" == "cli" ]]; then
        opts="-h --help -V --version -T --toml -J --json -C --csv -O --output"
        case "${prev}" in
            -O|--output)
                COMPREPLY=( $(compgen -f -- "${cur}") )
                return 0
                ;;
            *)
                ;;
        esac
    else
        opts="cli -h --help -V --version --system-font"
    fi

    COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
}
complete -F _cefdetector cefdetector
