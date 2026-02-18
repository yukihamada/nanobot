use rustyline::Editor;
use colored::*;
use std::io::{self, Write};

#[derive(Debug, Clone)]
enum Mode {
    Chat,
    Code,
    Pair,
    Command,
}

struct Cli {
    mode: Mode,
    editor: Editor<()>,
}

impl Cli {
    fn new() -> Self {
        Self {
            mode: Mode::Chat,
            editor: Editor::<()>::new().unwrap(),
        }
    }

    fn get_prompt(&self) -> String {
        match self.mode {
            Mode::Chat => ">> ".blue().to_string(),
            Mode::Code => "? ".green().to_string(),
            Mode::Pair => "! ".yellow().to_string(),
            Mode::Command => "/ ".red().to_string(),
        }
    }

    fn process_command(&mut self, cmd: &str) -> bool {
        match cmd.trim() {
            "/help" => {
                println!("{}", "Available commands:".green());
                println!("/help    - Show this help");
                println!("/mode    - Switch mode (chat/code/pair)");
                println!("/clear   - Clear screen");
                println!("/exit    - Exit program");
            }
            "/mode chat" => {
                self.mode = Mode::Chat;
                println!("Switched to chat mode");
            }
            "/mode code" => {
                self.mode = Mode::Code;
                println!("Switched to code mode");
            }
            "/mode pair" => {
                self.mode = Mode::Pair;
                println!("Switched to pair programming mode");
            }
            "/clear" => {
                print!("\x1B[2J\x1B[1;1H");
                io::stdout().flush().unwrap();
            }
            "/exit" => return false,
            _ => {
                if cmd.starts_with('/') {
                    println!("Unknown command. Type /help for available commands.");
                }
            }
        }
        true
    }

    fn run(&mut self) {
        println!("{}", "Welcome to nanobot CLI!".green());
        println!("Type /help for available commands");

        loop {
            let prompt = self.get_prompt();
            match self.editor.readline(&prompt) {
                Ok(line) => {
                    self.editor.add_history_entry(line.as_str());
                    
                    if line.starts_with('/') {
                        if !self.process_command(&line) {
                            break;
                        }
                    } else {
                        // Process normal input based on mode
                        match self.mode {
                            Mode::Chat => println!("Chat: {}", line),
                            Mode::Code => println!("Code: {}", line),
                            Mode::Pair => println!("Pair Programming: {}", line),
                            Mode::Command => println!("Command: {}", line),
                        }
                    }
                }
                Err(_) => break,
            }
        }
    }
}

fn main() {
    let mut cli = Cli::new();
    cli.run();
}