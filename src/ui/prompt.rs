use colored::*;
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;

pub struct Prompt {
    style: Style,
    completions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Style {
    prompt_char: String,
    color: Color,
}

#[derive(Debug, Clone)]
pub enum Color {
    Blue,
    Green,
    Yellow,
    Red,
}

impl Prompt {
    pub fn new() -> Self {
        Self {
            style: Style::default(),
            completions: vec![
                "help".to_string(),
                "context".to_string(),
                "clear".to_string(),
                "tokens".to_string(),
                "mode".to_string(),
                "save".to_string(),
                "load".to_string(),
                "config".to_string(),
                "version".to_string(),
            ],
        }
    }

    pub fn set_style(&mut self, style: Style) {
        self.style = style;
    }

    pub fn get_prompt(&self) -> String {
        match self.style.color {
            Color::Blue => self.style.prompt_char.blue().to_string(),
            Color::Green => self.style.prompt_char.green().to_string(),
            Color::Yellow => self.style.prompt_char.yellow().to_string(),
            Color::Red => self.style.prompt_char.red().to_string(),
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self {
            prompt_char: ">> ".to_string(),
            color: Color::Blue,
        }
    }
}

// Rustyline traits implementation
impl Completer for Prompt {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize) -> rustyline::Result<(usize, Vec<Pair>)> {
        let mut matches: Vec<Pair> = Vec::new();
        
        if line.starts_with('/') {
            let word = &line[1..pos];
            for cmd in &self.completions {
                if cmd.starts_with(word) {
                    matches.push(Pair {
                        display: cmd.clone(),
                        replacement: cmd.clone(),
                    });
                }
            }
        }

        Ok((if line.starts_with('/') { 1 } else { pos }, matches))
    }
}

impl Hinter for Prompt {
    type Hint = String;
}

impl Highlighter for Prompt {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        if line.starts_with('/') {
            return line.blue().to_string().into();
        }
        line.into()
    }
}

impl Validator for Prompt {}