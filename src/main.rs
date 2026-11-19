mod album;
mod banner;
mod cli;
mod discid;
mod drive;
mod musicbrainz;
mod rip;

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use console::{measure_text_width, style};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, Select};
use indicatif::{ProgressBar, ProgressStyle};

use crate::album::Album;
use crate::cli::Cli;
use crate::discid::Toc;
use crate::musicbrainz::Client;

fn main() -> ExitCode {
    let cli = Cli::parse();
    banner::print();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\n  {} {e:#}\n", style("error:").red().bold());
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    rip::check_tools(cli.format)?;
    let music_dir = music_dir(cli)?;
    let interactive = !cli.yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let client = Client::new();

    loop {
        let toc = drive::wait_for_disc(&cli.device)?;
        let result = rip_disc(cli, &client, &music_dir, &toc, interactive);

        if !cli.no_eject
            && let Err(e) = drive::eject(&cli.device)
        {
            println!("  {} {e:#}", style("!").yellow());
        }
        if !cli.watch {
            return result;
        }
        // In watch mode one bad disc shouldn't stop the whole session.
        if let Err(e) = result {
            println!("\n  {} {e:#}\n", style("error:").red().bold());
        }
        drive::wait_for_removal(&cli.device)?;
        println!("  {}\n", style("Ready for the next disc.").dim());
    }
}

fn music_dir(cli: &Cli) -> Result<PathBuf> {
    cli.music_dir
        .clone()
        .or_else(dirs::audio_dir)
        .or_else(|| dirs::home_dir().map(|home| home.join("Music")))
        .context("couldn't find your music folder, pass one with --music-dir")
}

fn spinner(message: &'static str) -> ProgressBar {
    let bar = ProgressBar::new_spinner().with_message(message);
    bar.set_style(
        ProgressStyle::with_template("  {spinner:.magenta} {msg}").expect("valid template"),
    );
    bar.enable_steady_tick(Duration::from_millis(100));
    bar
}

fn rip_disc(
    cli: &Cli,
    client: &Client,
    music_dir: &Path,
    toc: &Toc,
    interactive: bool,
) -> Result<()> {
    let started = Instant::now();
    let tracks = toc.audio_tracks().len();
    println!(
        "  {} Found a disc: {tracks} tracks, {}  {}",
        style("●").magenta(),
        duration(toc.total_seconds()),
        style(toc.disc_id()).dim()
    );

    let bar = spinner("Looking it up on MusicBrainz…");
    let releases = client.lookup(toc);
    bar.finish_and_clear();
    let releases = releases.unwrap_or_else(|e| {
        println!("  {} {e:#}", style("!").yellow());
        Vec::new()
    });

    let album = if releases.is_empty() {
        unknown_album(toc, interactive)?
    } else {
        let index = if releases.len() > 1 && interactive {
            let labels: Vec<String> = releases.iter().map(|r| r.label(toc)).collect();
            Select::with_theme(&theme())
                .with_prompt("Which release is yours?")
                .items(&labels)
                .default(0)
                .interact()?
        } else {
            0
        };
        Album::from_release(&releases[index], toc)
    };

    let folder = album.folder(music_dir);
    print_album(&album, cli, &folder);

    if interactive
        && !Confirm::with_theme(&theme())
            .with_prompt("Start ripping?")
            .default(true)
            .interact()?
    {
        println!("  Skipped.");
        return Ok(());
    }
    println!();

    fs::create_dir_all(&folder).with_context(|| format!("couldn't create {}", folder.display()))?;
    let temp = tempfile::Builder::new()
        .prefix("cdrip-")
        .tempdir()
        .context("couldn't create a temporary folder")?;

    // Keep a copy of the cover in the temp folder for the encoders: its path is simple,
    // which matters because flac/opusenc treat `|` in picture arguments specially.
    let cover = album.release_id.as_deref().and_then(|id| {
        let bar = spinner("Fetching cover art…");
        let image = client.front_cover(id);
        bar.finish_and_clear();
        let image = image?;
        let embedded = temp.path().join("cover.jpg");
        fs::write(&embedded, &image).ok()?;
        let in_folder = folder.join("cover.jpg");
        if !in_folder.exists() {
            let _ = fs::write(in_folder, &image);
        }
        println!("  {} cover art", style("✔").green());
        Some(embedded)
    });

    let summary = rip::rip_album(&rip::Job {
        device: &cli.device,
        format: cli.format,
        album: &album,
        folder: &folder,
        cover: cover.as_deref(),
        temp: temp.path(),
    });

    println!();
    if summary.skipped > 0 {
        println!(
            "  {} {} track(s) were already ripped, skipped them",
            style("↷").dim(),
            summary.skipped
        );
    }
    if summary.failed.is_empty() {
        println!(
            "  {} Done! {} track(s) in {}",
            style("✔").green().bold(),
            summary.ripped,
            duration(u32::try_from(started.elapsed().as_secs()).unwrap_or(u32::MAX)),
        );
        println!("  {} {}\n", style("→").cyan(), pretty_path(&folder));
        Ok(())
    } else {
        anyhow::bail!(
            "{} track(s) failed: {}. Clean the disc and run cdrip again to retry just those.",
            summary.failed.len(),
            summary.failed.join(", ")
        )
    }
}

fn unknown_album(toc: &Toc, interactive: bool) -> Result<Album> {
    println!(
        "  {} MusicBrainz doesn't know this disc yet. You can add it here:\n    {}",
        style("?").yellow(),
        style(toc.submit_url()).underlined()
    );
    if !interactive {
        let id = toc.disc_id();
        return Ok(Album::unknown(
            toc,
            "Unknown Artist".into(),
            format!("Unknown Album {}", &id[..8]),
        ));
    }
    let theme = theme();
    let artist: String = Input::with_theme(&theme)
        .with_prompt("Artist")
        .default("Unknown Artist".into())
        .interact_text()?;
    let title: String = Input::with_theme(&theme)
        .with_prompt("Album")
        .default("Unknown Album".into())
        .interact_text()?;
    Ok(Album::unknown(toc, artist, title))
}

/// Dialoguer's colorful theme, indented to line up with the rest of the output.
fn theme() -> ColorfulTheme {
    ColorfulTheme {
        prompt_prefix: style("  ?".into()).yellow().bold(),
        success_prefix: style("  ✔".into()).green(),
        error_prefix: style("  ✘".into()).red(),
        active_item_prefix: style("  ❯".into()).cyan(),
        inactive_item_prefix: style("   ".into()),
        ..ColorfulTheme::default()
    }
}

fn print_album(album: &Album, cli: &Cli, folder: &Path) {
    let year = album
        .year
        .as_deref()
        .map(|y| format!(" ({y})"))
        .unwrap_or_default();
    let disc = if album.disc_total > 1 {
        format!(" · disc {}/{}", album.disc, album.disc_total)
    } else {
        String::new()
    };
    println!(
        "\n  {} — {}{year}{disc}",
        style(&album.artist).bold(),
        style(&album.title).cyan().bold()
    );
    let label = |track: &album::Track| {
        if track.artist == album.artist {
            track.title.clone()
        } else {
            format!("{} · {}", track.title, track.artist)
        }
    };
    let width = album
        .tracks
        .iter()
        .map(|t| measure_text_width(&label(t)))
        .max()
        .unwrap_or(0);
    for track in &album.tracks {
        let label = label(track);
        let padding = " ".repeat(width - measure_text_width(&label));
        println!(
            "   {}  {label}{padding}  {}",
            style(format!("{:02}", track.position)).dim(),
            style(duration(track.audio.seconds())).dim()
        );
    }
    println!(
        "\n  {} {} → {}\n",
        style("◆").magenta(),
        cli.format.label(),
        style(pretty_path(folder)).underlined()
    );
}

/// `/home/you/Music/x` → `~/Music/x`
fn pretty_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// 3725 → "1:02:05", 225 → "3:45"
fn duration(seconds: u32) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_durations() {
        assert_eq!(super::duration(225), "3:45");
        assert_eq!(super::duration(3725), "1:02:05");
    }
}
