_cefdetector() {
    local cur prev opts
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    opts="-h --help -V --version -T --toml -J --json -C --csv -O --output --system-font"

    case "${prev}" in
        -O|--output)
            COMPREPLY=( $(compgen -f -- "${cur}") )
            return 0
            ;;
        *)
            ;;
    esac

    if [[ ${cur} == -* ]] ; then
        COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
        return 0
    fi
}
complete -F _cefdetector cefdetector
