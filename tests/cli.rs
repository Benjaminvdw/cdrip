//! Tests for command-line argument parsing.
//!
//! `Cli::try_parse_from` parses a list of strings instead of the real
//! command line, and returns an error instead of exiting the program.
//! That makes the CLI testable without actually running the binary.

use std::path::PathBuf;

use cdrip::cli::{Cli, Command, Format};
use clap::{CommandFactory, Parser};

/// Parse `args` as if they were typed after `cdrip` in a terminal.
fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    let full_args = std::iter::once("cdrip").chain(args.iter().copied());
    Cli::try_parse_from(full_args)
}

#[test]
fn cli_definition_is_valid() {
    // clap's own self-check: catches mistakes like two args with the same name.
    Cli::command().debug_assert();
}

#[test]
fn rip_uses_defaults() {
    let cli = parse(&["rip"]).expect("`rip` should parse");

    assert_eq!(cli.device, PathBuf::from("/dev/sr0"));
    let Command::Rip { output, format } = cli.command else {
        panic!("expected the rip command, got {:?}", cli.command);
    };
    assert_eq!(output, None);
    assert_eq!(format, Format::Flac);
}

#[test]
fn rip_accepts_format_and_output() {
    let cli = parse(&["rip", "--format", "opus", "--output", "/tmp/music"])
        .expect("valid rip arguments should parse");

    let Command::Rip { output, format } = cli.command else {
        panic!("expected the rip command, got {:?}", cli.command);
    };
    assert_eq!(output, Some(PathBuf::from("/tmp/music")));
    assert_eq!(format, Format::Opus);
}

#[test]
fn device_can_come_after_the_subcommand() {
    let cli = parse(&["toc", "--device", "/dev/sr1"]).expect("`toc --device` should parse");

    assert_eq!(cli.device, PathBuf::from("/dev/sr1"));
    assert!(matches!(cli.command, Command::Toc));
}

#[test]
fn unknown_format_is_rejected() {
    assert!(parse(&["rip", "--format", "wav"]).is_err());
}

#[test]
fn a_subcommand_is_required() {
    assert!(parse(&[]).is_err());
}
