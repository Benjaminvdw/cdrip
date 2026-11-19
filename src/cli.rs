use std::path::PathBuf;

use clap::{Parser, ValueEnum};

/// Rip audio CDs straight into your music library.
///
/// Just run `cdrip`: it waits for a disc, looks it up on MusicBrainz, rips and
/// encodes every track, adds tags and cover art, and ejects the disc when done.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Music library folder. Albums go into "Artist/Album (Year)" inside it.
    /// Defaults to your XDG music folder, usually ~/Music.
    #[arg(short = 'o', long, env = "CDRIP_MUSIC_DIR", value_name = "DIR")]
    pub music_dir: Option<PathBuf>,

    /// Audio format to encode to.
    #[arg(short, long, value_enum, default_value_t = Format::Flac, env = "CDRIP_FORMAT")]
    pub format: Format,

    /// CD drive to read from.
    #[arg(short, long, default_value = "/dev/sr0", env = "CDRIP_DEVICE")]
    pub device: PathBuf,

    /// Keep going: after each disc, eject and wait for the next one.
    #[arg(short, long)]
    pub watch: bool,

    /// Don't ask anything; pick the first MusicBrainz match and start ripping.
    #[arg(short, long)]
    pub yes: bool,

    /// Leave the disc in the drive when finished.
    #[arg(long)]
    pub no_eject: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Lossless, best quality (default)
    Flac,
    /// MP3 V0 (~245 kbps VBR), plays everywhere
    Mp3,
    /// Opus 160 kbps, small files, great quality
    Opus,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Flac => "flac",
            Format::Mp3 => "mp3",
            Format::Opus => "opus",
        }
    }

    /// The encoder program this format needs, and the package that provides it.
    pub fn encoder(self) -> (&'static str, &'static str) {
        match self {
            Format::Flac => ("flac", "flac"),
            Format::Mp3 => ("lame", "lame"),
            Format::Opus => ("opusenc", "opus-tools"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Flac => "FLAC",
            Format::Mp3 => "MP3",
            Format::Opus => "Opus",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults() {
        let cli = Cli::try_parse_from(["cdrip"]).unwrap();
        assert_eq!(cli.format, Format::Flac);
        assert!(!cli.watch && !cli.yes && !cli.no_eject);
    }

    #[test]
    fn parses_flags() {
        let cli = Cli::try_parse_from(["cdrip", "-f", "opus", "-w", "-y", "-o", "/tmp/m"]).unwrap();
        assert_eq!(cli.format, Format::Opus);
        assert!(cli.watch && cli.yes);
        assert_eq!(cli.music_dir, Some(PathBuf::from("/tmp/m")));
    }
}
