use std::io::{self, Write};
use std::fs::File;
use std::io::{BufRead, BufReader};
use helios_shared::{log_info, log_error};

mod parser;
mod exec;
mod jobs;
mod signals;

use parser::{Lexer, Parser};
use exec::{execute_command, ShellState};
use jobs::JobTable;
use signals::init_shell_signals;

fn main() {
    unsafe {
        // 1. Initialize signals in parent shell process (ignores Ctrl+C, Ctrl+Z, etc.)
        init_shell_signals();
    }

    let shell_pgid = nix::unistd::getpgrp();
    let mut job_table = JobTable::new(shell_pgid);
    let mut state = ShellState::new();

    let args: Vec<String> = std::env::args().collect();

    // 2. Scripting Support: If executed with an argument, read and execute that file
    if args.len() > 1 {
        let script_path = &args[1];
        if let Err(e) = execute_script_file(script_path, &mut state, &mut job_table) {
            eprintln!("helios: failed to execute script '{}': {}", script_path, e);
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    // 3. Interactive REPL Loop
    println!("---------------------------------------------------------------");
    println!("     Welcome to Helios Advanced Unix Shell (Rust-Powered)     ");
    println!("---------------------------------------------------------------");
    println!("Type 'exit' to quit, 'jobs' to list background tasks.");

    loop {
        unsafe {
            // Asynchronously poll and harvest completed background jobs
            job_table.check_background_jobs();
            job_table.clean_completed_jobs();
        }

        // Display highly educational, beautiful ANSI-colored prompt
        print_colored_prompt(&state);
        let _ = io::stdout().flush();

        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(0) => {
                // EOF (Ctrl+D)
                println!();
                log_info!("shell", "EOF received. Exiting shell cleanly.");
                break;
            }
            Ok(_) => {
                let trimmed = input.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Add to history
                state.history.push(trimmed.to_string());

                // Tokenize command line
                let mut lexer = Lexer::new(trimmed);
                let tokens_res = lexer.tokenize(&state.aliases);
                let tokens = match tokens_res {
                    Ok(t) => t,
                    Err(e) => {
                        println!("helios: lexer error: {}", e);
                        continue;
                    }
                };

                if tokens.is_empty() {
                    continue;
                }

                // Parse tokens into command AST
                let mut parser = Parser::new(tokens);
                let command = match parser.parse() {
                    Ok(c) => c,
                    Err(e) => {
                        println!("helios: syntax error: {}", e);
                        continue;
                    }
                };

                // Execute Command AST
                unsafe {
                    execute_command(&command, &mut state, &mut job_table, true, trimmed);
                }
            }
            Err(e) => {
                log_error!("shell", "Stdin read error: {}", e);
                break;
            }
        }
    }
}

/// Print beautiful premium prompt using ANSI codes
fn print_colored_prompt(state: &ShellState) {
    let dir_str = state.current_dir.to_string_lossy();
    
    // Purple "helios", bright green arrow, cyan directory layout, white dollar sign
    print!("\x1b[1;35mhelios\x1b[0m \x1b[1;32m➜\x1b[0m \x1b[1;36m[{}]\x1b[0m \x1b[1;37m$\x1b[0m ", dir_str);
}

/// Script execution helper for running .sh/.helios files
fn execute_script_file(path: &str, state: &mut ShellState, job_table: &mut JobTable) -> io::Result<()> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line_content = line?;
        let trimmed = line_content.trim();
        
        // Skip empty lines or comment blocks
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Lex line
        let mut lexer = Lexer::new(trimmed);
        let tokens = match lexer.tokenize(&state.aliases) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("helios script: tokenization error: {}", e);
                continue;
            }
        };

        if tokens.is_empty() {
            continue;
        }

        // Parse line
        let mut parser = Parser::new(tokens);
        let command = match parser.parse() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("helios script: parse error: {}", e);
                continue;
            }
        };

        // Run line
        unsafe {
            execute_command(&command, state, job_table, true, trimmed);
        }
    }

    Ok(())
}
