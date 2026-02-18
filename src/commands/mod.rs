use crate::cli::Mode;
use colored::*;

pub struct Command {
    pub name: String,
    pub description: String,
    pub handler: Box<dyn Fn() -> Result<(), String>>,
}

pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn register(&mut self, name: &str, description: &str, handler: Box<dyn Fn() -> Result<(), String>>) {
        self.commands.push(Command {
            name: name.to_string(),
            description: description.to_string(),
            handler,
        });
    }

    pub fn execute(&self, name: &str) -> Result<(), String> {
        if let Some(command) = self.commands.iter().find(|c| c.name == name) {
            (command.handler)()
        } else {
            Err(format!("Unknown command: {}", name))
        }
    }

    pub fn list_commands(&self) {
        println!("\n{}", "Available Commands:".bold());
        for cmd in &self.commands {
            println!("{:<15} - {}", cmd.name.blue(), cmd.description);
        }
        println!();
    }
}

// Default commands
pub fn register_default_commands(registry: &mut CommandRegistry) {
    registry.register("help", 
        "Show this help",
        Box::new(|| {
            println!("Help information");
            Ok(())
        })
    );

    registry.register("context",
        "Show current context",
        Box::new(|| {
            println!("Current context");
            Ok(())
        })
    );

    // Add more default commands...
}