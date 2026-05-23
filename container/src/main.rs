use std::env;
use std::fs;
use std::path::Path;
use helios_shared::{log_error, log_info};

mod container;
mod namespaces;
mod container_fs;
mod cgroups;
mod seccomp;

use container::{Container, ContainerConfig, ContainerStatus};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    let subcommand = &args[1];
    match subcommand.as_str() {
        "run" => {
            if args.len() < 4 {
                println!("Error: 'run' requires container ID and config options.");
                print_run_usage();
                std::process::exit(1);
            }
            let id = &args[2];
            let mut rootfs = String::new();
            let mut command = Vec::new();
            let mut config_file = String::new();
            let mut memory_limit = None;
            let mut cpu_limit = None;

            // Simple robust CLI parsing loop
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--rootfs" => {
                        if i + 1 < args.len() {
                            rootfs = args[i + 1].clone();
                            i += 2;
                        } else {
                            println!("Error: --rootfs requires a path value.");
                            std::process::exit(1);
                        }
                    }
                    "--command" => {
                        if i + 1 < args.len() {
                            // Support multi-word commands separated by spaces
                            command = args[i + 1].split_whitespace().map(|s| s.to_string()).collect();
                            i += 2;
                        } else {
                            println!("Error: --command requires a string value.");
                            std::process::exit(1);
                        }
                    }
                    "--config" => {
                        if i + 1 < args.len() {
                            config_file = args[i + 1].clone();
                            i += 2;
                        } else {
                            println!("Error: --config requires a filepath value.");
                            std::process::exit(1);
                        }
                    }
                    "--memory" => {
                        if i + 1 < args.len() {
                            if let Ok(bytes) = args[i + 1].parse::<usize>() {
                                memory_limit = Some(bytes);
                            } else {
                                println!("Error: Invalid memory value.");
                                std::process::exit(1);
                            }
                            i += 2;
                        } else {
                            println!("Error: --memory requires a byte value.");
                            std::process::exit(1);
                        }
                    }
                    "--cpu" => {
                        if i + 1 < args.len() {
                            if let Ok(pct) = args[i + 1].parse::<usize>() {
                                cpu_limit = Some(pct);
                            } else {
                                println!("Error: Invalid CPU value.");
                                std::process::exit(1);
                            }
                            i += 2;
                        } else {
                            println!("Error: --cpu requires a percentage value.");
                            std::process::exit(1);
                        }
                    }
                    _ => {
                        println!("Error: Unknown option '{}'", args[i]);
                        print_run_usage();
                        std::process::exit(1);
                    }
                }
            }

            // Determine config structure
            let config = if !config_file.is_empty() {
                // Load from OCI config JSON
                let data = match fs::read_to_string(&config_file) {
                    Ok(d) => d,
                    Err(e) => {
                        log_error!("container", "Failed to read config JSON file: {}", e);
                        std::process::exit(1);
                    }
                };
                match serde_json::from_str::<ContainerConfig>(&data) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        log_error!("container", "Failed to parse config JSON: {}", e);
                        std::process::exit(1);
                    }
                }
            } else {
                if rootfs.is_empty() || command.is_empty() {
                    println!("Error: Must provide --rootfs and --command, or --config JSON.");
                    print_run_usage();
                    std::process::exit(1);
                }
                ContainerConfig {
                    hostname: id.clone(),
                    rootfs,
                    command,
                    memory_limit_bytes: memory_limit,
                    cpu_limit_percentage: cpu_limit,
                }
            };

            // Instantiate and execute container lifecycle
            let container = Container::new(id);
            unsafe {
                if let Err(e) = container.run(config) {
                    log_error!("container", "Container run failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "stop" => {
            if args.len() < 3 {
                println!("Error: 'stop' requires a container ID");
                std::process::exit(1);
            }
            let id = &args[2];
            let container = Container::new(id);
            if let Err(e) = container.stop() {
                log_error!("container", "Stop failed: {}", e);
                std::process::exit(1);
            }
        }
        "delete" => {
            if args.len() < 3 {
                println!("Error: 'delete' requires a container ID");
                std::process::exit(1);
            }
            let id = &args[2];
            let container = Container::new(id);
            if let Err(e) = container.destroy() {
                log_error!("container", "Deletion failed: {}", e);
                std::process::exit(1);
            }
        }
        "state" => {
            if args.len() < 3 {
                println!("Error: 'state' requires a container ID");
                std::process::exit(1);
            }
            let id = &args[2];
            let container = Container::new(id);
            match container.load_state() {
                Ok(state) => {
                    let pretty = serde_json::to_string_pretty(&state).unwrap();
                    println!("{}", pretty);
                }
                Err(e) => {
                    log_error!("container", "Failed to retrieve state: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "list" => {
            match Container::list() {
                Ok(list) => {
                    println!("===============================================================================");
                    println!("{:<16} {:<8} {:<10} {:<15} {:<15}", "CONTAINER ID", "PID", "STATUS", "CREATED AT", "COMMAND");
                    println!("-------------------------------------------------------------------------------");
                    for c in list {
                        let pid_str = c.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
                        let status_str = match c.status {
                            ContainerStatus::Creating => "Creating",
                            ContainerStatus::Running => "Running",
                            ContainerStatus::Stopped => "Stopped",
                        };
                        // Shorten command representation
                        let cmd_str = c.config.command.join(" ");
                        let cmd_short = if cmd_str.len() > 20 {
                            format!("{}...", &cmd_str[..17])
                        } else {
                            cmd_str
                        };
                        println!("{:<16} {:<8} {:<10} {:<15} {:<15}", c.id, pid_str, status_str, "Active", cmd_short);
                    }
                    println!("===============================================================================");
                }
                Err(e) => {
                    log_error!("container", "Failed to list containers: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "help" | "--help" | "-h" => {
            print_usage();
        }
        _ => {
            println!("Error: Unknown subcommand '{}'", subcommand);
            print_usage();
            std::process::exit(1);
        }
    }
}

fn print_usage() {
    println!("Helios Container Runtime CLI");
    println!("Usage:");
    println!("  helios-container <subcommand> [args]");
    println!();
    println!("Subcommands:");
    println!("  run <id> --rootfs <path> --command \"<cmd>\"   Create and run an isolated container");
    println!("  stop <id>                                    Stop a running container process");
    println!("  delete <id>                                  Destroy container cgroups and states");
    println!("  state <id>                                   Retrieve JSON status descriptor");
    println!("  list                                         List tracked container structures");
    println!();
    println!("Run options:");
    println!("  --rootfs <path>         Path to root filesystem folder");
    println!("  --command \"<cmd>\"       Command and args to execute inside container");
    println!("  --config <file.json>    Load OCI config settings directly from JSON file");
    println!("  --memory <bytes>        Maximum memory allocation limit");
    println!("  --cpu <percentage>      Throttled CPU share quota");
}

fn print_run_usage() {
    println!("Usage:");
    println!("  helios-container run <id> --rootfs <path> --command \"<cmd>\" [--memory <bytes>] [--cpu <percentage>]");
    println!("  helios-container run <id> --config <file.json>");
}
