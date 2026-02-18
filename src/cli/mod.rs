use std::io::{self, Write};
use colored::*;
use rustyline::Editor;

pub struct Cli {
    editor: Editor<()>,
    mode: Mode,
    context: Context,
}

#[derive(Debug, Clone)]
pub enum Mode {
    Chat,
    Code,
    Pair,
}

#[derive(Debug, Clone)]
pub struct Context {
    current_dir: String,
    files_in_focus: Vec<String>,
    memory_usage: usize,
    token_count: usize,
}

impl Cli {
    pub fn new() -> Self {
        Self {
            editor: Editor::<()>::new().unwrap(),
            mode: Mode::Chat,
            context: Context::default(),
        }
    }

    pub fn run(&mut self) -> io::Result<()> {
        println!("{}", "nanobot v2.0.0".bold().green());
        println!("Type {} for help", "/help".blue());

        loop {
            let prompt = match self.mode {
                Mode::Chat => ">> ",
                Mode::Code => "? ",
                Mode::Pair => "! ",
            };

            match self.editor.readline(prompt) {
                Ok(line) => {
                    self.editor.add_history_entry(line.clone());
                    self.handle_input(&line)?;
                }
                Err(_) => break,
            }
        }
        Ok(())
    }

    fn handle_input(&mut self, input: &str) -> io::Result<()> {
        if input.starts_with('/') {
            self.handle_command(&input[1..])?;
        } else {
            self.handle_normal_input(input)?;
        }
        Ok(())
    }

    fn handle_command(&mut self, cmd: &str) -> io::Result<()> {
        match cmd {
            "help" => self.show_help(),
            "context" => self.show_context(),
            "clear" => self.clear_context(),
            "tokens" => self.show_tokens(),
            "mode chat" => self.set_mode(Mode::Chat),
            "mode code" => self.set_mode(Mode::Code),
            "mode pair" => self.set_mode(Mode::Pair),
            "save" => self.save_state(),
            "load" => self.load_state(),
            "config" => self.show_config(),
            "version" => self.show_version(),
            _ => println!("Unknown command. Type {} for help", "/help".blue()),
        }
        Ok(())
    }

    fn show_help(&self) {
        println!("\n{}", "Available Commands:".bold());
        println!("{:<15} - Show this help", "/help".blue());
        println!("{:<15} - Show current context", "/context".blue());
        println!("{:<15} - Clear current context", "/clear".blue());
        println!("{:<15} - Show token usage", "/tokens".blue());
        println!("{:<15} - Switch to chat mode", "/mode chat".blue());
        println!("{:<15} - Switch to code mode", "/mode code".blue());
        println!("{:<15} - Switch to pair programming mode", "/mode pair".blue());
        println!("{:<15} - Save current state", "/save".blue());
        println!("{:<15} - Load saved state", "/load".blue());
        println!("{:<15} - Show/edit config", "/config".blue());
        println!("{:<15} - Show version info", "/version".blue());
        println!();
    }

    // Other methods implementation...
}

impl Default for Context {
    fn default() -> Self {
        Self {
            current_dir: String::from("."),
            files_in_focus: Vec::new(),
            memory_usage: 0,
            token_count: 0,
        }
    }
}