//! Command-line argument definitions (you will fill this in).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

///Rip audio cds into tagged album folders
#[derive(Parser, Debug)]
#[command(version)]
pub struct Cli {
    /// cd drive to read from
    #[arg(long, global = true, default_value = "/dev/sr0")]
    pub device: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// rip the inserted cd onto a tagged album folder
    Rip {
        /// folder to put albums in
        #[arg(long)]
        output: Option<PathBuf>,

        /// audio format to encode to
        #[arg(long, value_enum, default_value_t = Format::Flac)]
        format: Format,
    },
    /// print the disc table of contents
    Toc,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Flac,

    Opus,

    Mp3,
}
