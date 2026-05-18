_mae_state_server() {
    local cur prev opts commands
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    commands="start doctor"
    opts="--bind --unix-socket --config --data-dir --compact-threshold --check-config --version --help"

    case "${prev}" in
        --bind|-b)
            COMPREPLY=( $(compgen -W "127.0.0.1:9473 0.0.0.0:9473" -- "${cur}") )
            return 0
            ;;
        --config|-c|--data-dir|-d|--unix-socket|-u)
            COMPREPLY=( $(compgen -f -- "${cur}") )
            return 0
            ;;
    esac

    if [[ ${COMP_CWORD} -eq 1 ]]; then
        COMPREPLY=( $(compgen -W "${commands} ${opts}" -- "${cur}") )
    else
        COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
    fi
}
complete -F _mae_state_server mae-state-server
