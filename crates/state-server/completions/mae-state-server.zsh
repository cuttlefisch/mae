#compdef mae-state-server

_mae_state_server() {
    local -a commands opts
    commands=(
        'start:Start the state server'
        'doctor:Run diagnostics'
    )
    opts=(
        '--bind[TCP bind address]:addr:(127.0.0.1\:9473 0.0.0.0\:9473)'
        '--unix-socket[Unix socket path]:path:_files'
        '--config[Config file path]:path:_files'
        '--data-dir[Data directory]:path:_directories'
        '--compact-threshold[WAL compaction threshold]:count:'
        '--check-config[Validate config and exit]'
        '--version[Print version]'
        '--help[Show help]'
    )

    _arguments -s \
        '1:command:->command' \
        '*:option:->option'

    case $state in
        command)
            _describe 'command' commands
            _describe 'option' opts
            ;;
        option)
            _values 'option' $opts
            ;;
    esac
}

_mae_state_server "$@"
