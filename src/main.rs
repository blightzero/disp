mod config;
mod display;
mod error;
mod cli;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Run the CLI
    cli::run()?;
    
    Ok(())
}
