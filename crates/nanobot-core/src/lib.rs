pub mod error;
pub mod types;
pub mod config;
pub mod bus;
pub mod session;
pub mod memory;
pub mod skills;
pub mod agent;
pub mod provider;
pub mod tool;
pub mod channel;
pub mod service;
pub mod util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const LOGO: &str = "🐈";
