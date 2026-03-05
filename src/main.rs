//! ZeptoClaw CLI — Ultra-lightweight personal AI assistant
//!
//! All CLI logic lives in the `cli` module. This file is just the entry point.

mod cli;

#[tokio::main]
async fn main() {
    match cli::run().await {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}
