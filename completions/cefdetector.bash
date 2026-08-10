_cefdetector() {
    local cur prev opts command word
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    case "${prev}" in
        --config|-O|--output)
            COMPREPLY=( $(compgen -f -- "${cur}") )
            return 0
            ;;
    esac

    for word in "${COMP_WORDS[@]:1}"; do
        if [[ "${word}" == "cli" || "${word}" == "config" ]]; then
            command="${word}"
            break
        fi
    done

    case "${command}" in
        cli)
            opts="-h --help -V --version -T --toml -J --json -C --csv -O --output --config --no-system-config --no-user-config --set"
            ;;
        config)
            if [[ "${prev}" == "validate" ]]; then
                COMPREPLY=( $(compgen -f -- "${cur}") )
                return 0
            fi
            opts="paths show validate -h --help --config --no-system-config --no-user-config --set"
            ;;
        *)
            opts="cli config -h --help -V --version --config --no-system-config --no-user-config --set"
            ;;
    esac

    COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
}
complete -F _cefdetector cefdetector
