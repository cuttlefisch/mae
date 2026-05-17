//! Command-line argument parsing for mae-state-server.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Parsed CLI arguments.
pub struct CliArgs {
    pub command: Command,
}

/// Top-level command.
pub enum Command {
    /// Start the state server (default).
    Start(StartArgs),
    /// Check configuration and exit.
    CheckConfig,
    /// Run diagnostics.
    Doctor,
    /// Print version.
    Version,
}

/// Arguments for the `start` subcommand.
pub struct StartArgs {
    /// TCP bind address (default: 127.0.0.1:9473).
    pub bind: SocketAddr,
    /// Optional Unix socket path for local clients.
    pub unix_socket: Option<PathBuf>,
    /// Path to state-server.toml config file.
    pub config: Option<PathBuf>,
    /// Data directory for SQLite storage.
    pub data_dir: Option<PathBuf>,
    /// WAL compaction threshold (updates per document).
    pub compact_threshold: u64,
}

impl Default for StartArgs {
    fn default() -> Self {
        StartArgs {
            bind: "127.0.0.1:9473".parse().unwrap(),
            unix_socket: None,
            config: None,
            data_dir: None,
            compact_threshold: 500,
        }
    }
}

pub fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        return CliArgs {
            command: Command::Start(StartArgs::default()),
        };
    }

    match args[1].as_str() {
        "--version" | "-V" => CliArgs {
            command: Command::Version,
        },
        "--check-config" => CliArgs {
            command: Command::CheckConfig,
        },
        "doctor" => CliArgs {
            command: Command::Doctor,
        },
        "start" => CliArgs {
            command: Command::Start(parse_start_args(&args[2..])),
        },
        _ => CliArgs {
            command: Command::Start(parse_start_args(&args[1..])),
        },
    }
}

fn parse_start_args(args: &[String]) -> StartArgs {
    let mut result = StartArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--bind" | "-b" => {
                if i + 1 < args.len() {
                    if let Ok(addr) = args[i + 1].parse() {
                        result.bind = addr;
                    } else {
                        eprintln!("error: invalid bind address: {}", args[i + 1]);
                        std::process::exit(1);
                    }
                    i += 2;
                } else {
                    eprintln!("error: --bind requires an argument");
                    std::process::exit(1);
                }
            }
            "--unix-socket" | "-u" => {
                if i + 1 < args.len() {
                    result.unix_socket = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    eprintln!("error: --unix-socket requires an argument");
                    std::process::exit(1);
                }
            }
            "--config" | "-c" => {
                if i + 1 < args.len() {
                    result.config = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    eprintln!("error: --config requires an argument");
                    std::process::exit(1);
                }
            }
            "--data-dir" | "-d" => {
                if i + 1 < args.len() {
                    result.data_dir = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    eprintln!("error: --data-dir requires an argument");
                    std::process::exit(1);
                }
            }
            "--compact-threshold" => {
                if i + 1 < args.len() {
                    result.compact_threshold = args[i + 1].parse().unwrap_or(500);
                    i += 2;
                } else {
                    eprintln!("error: --compact-threshold requires an argument");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                eprintln!("error: unknown option: {}", other);
                eprintln!("hint: run `mae-state-server --help` for usage");
                std::process::exit(1);
            }
        }
    }
    result
}

fn print_help() {
    eprintln!(
        "mae-state-server {} — MAE collaborative state server

USAGE:
    mae-state-server [COMMAND] [OPTIONS]

COMMANDS:
    start              Start the state server (default)
    doctor             Run diagnostics
    --check-config     Validate configuration and exit
    --version, -V      Print version

OPTIONS (start):
    --bind, -b ADDR           TCP bind address [default: 127.0.0.1:9473]
    --unix-socket, -u PATH    Also listen on Unix socket
    --config, -c PATH         Config file path
    --data-dir, -d PATH       Data directory for SQLite
    --compact-threshold N     WAL compaction threshold [default: 500]
    --help, -h                Show this help",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_start_args() {
        let args = StartArgs::default();
        assert_eq!(args.bind.port(), 9473);
        assert!(args.unix_socket.is_none());
        assert_eq!(args.compact_threshold, 500);
    }

    #[test]
    fn parse_bind_flag() {
        let args = parse_start_args(&["--bind".to_string(), "0.0.0.0:8080".to_string()]);
        assert_eq!(args.bind.port(), 8080);
    }
}
