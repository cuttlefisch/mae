complete -c mae-state-server -n '__fish_use_subcommand' -a 'start' -d 'Start the state server'
complete -c mae-state-server -n '__fish_use_subcommand' -a 'doctor' -d 'Run diagnostics'
complete -c mae-state-server -l bind -s b -d 'TCP bind address' -x -a '127.0.0.1:9473 0.0.0.0:9473'
complete -c mae-state-server -l unix-socket -s u -d 'Unix socket path' -r -F
complete -c mae-state-server -l config -s c -d 'Config file path' -r -F
complete -c mae-state-server -l data-dir -s d -d 'Data directory' -r -F
complete -c mae-state-server -l compact-threshold -d 'WAL compaction threshold' -x
complete -c mae-state-server -l check-config -d 'Validate config and exit'
complete -c mae-state-server -l version -s V -d 'Print version'
complete -c mae-state-server -l help -s h -d 'Show help'
