use clap::Parser;

use cdrip::cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::Rip { output, format } => {
            println!("Would rip from{}", cli.device.display());
            println!("  format: {format:?}");
            println!("  output: {output:?}");
        }
        Command::Toc => {
            println!("Would read the TOC from {}", cli.device.display());
        }
    }
}
