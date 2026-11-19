//! Ripping with cdparanoia and encoding with flac / lame / opusenc.
//! While one track is being ripped, the previous one is encoded in the background.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use crate::album::{Album, Track};
use crate::cli::Format;

/// cdparanoia reports positions in 16-bit words; one CD sector holds 1176 of them.
const WORDS_PER_SECTOR: u64 = 1176;

pub struct Job<'a> {
    pub device: &'a Path,
    pub format: Format,
    pub album: &'a Album,
    pub folder: &'a Path,
    pub cover: Option<&'a Path>,
    pub temp: &'a Path,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub ripped: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
}

/// Is `program` somewhere on `$PATH`?
pub fn installed(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Make sure cdparanoia and the encoder are installed before we start.
pub fn check_tools(format: Format) -> Result<()> {
    let (encoder, package) = format.encoder();
    let missing: Vec<&str> = [("cdparanoia", "cdparanoia"), (encoder, package)]
        .into_iter()
        .filter(|(program, _)| !installed(program))
        .map(|(_, package)| package)
        .collect();
    if !missing.is_empty() {
        bail!(
            "missing tools: {0}\n  install them with:  sudo pacman -S {0}   (or apt/dnf install {0})",
            missing.join(" ")
        );
    }
    Ok(())
}

pub fn rip_album(job: &Job) -> Summary {
    let mut summary = Summary::default();
    let extension = job.format.extension();

    let todo: Vec<&Track> = job
        .album
        .tracks
        .iter()
        .filter(|track| {
            let done = job
                .folder
                .join(job.album.file_name(track, extension))
                .exists();
            if done {
                summary.skipped += 1;
            }
            !done
        })
        .collect();
    if todo.is_empty() {
        return summary;
    }

    let total: u64 = todo.iter().map(|t| u64::from(t.audio.sectors)).sum();
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::with_template(
            "  {spinner:.magenta} [{bar:32.cyan/blue}] {percent:>3}%  {msg}  {elapsed_precise:.dim}",
        )
        .expect("valid template")
        .progress_chars("━╸ "),
    );
    bar.enable_steady_tick(Duration::from_millis(100));

    let mut encoders: Vec<(String, JoinHandle<Result<()>>)> = Vec::new();
    let mut done_sectors = 0;
    for track in todo {
        let label = format!("{:02} {}", track.position, track.title);
        bar.set_message(format!("ripping {label}"));
        let wav = job.temp.join(format!("{:02}.wav", track.position));

        if let Err(e) = rip_track(job.device, track, &wav, &bar, done_sectors) {
            bar.println(format!("  {} {label}: {e:#}", style("✘").red()));
            summary.failed.push(label);
        } else {
            bar.println(format!("  {} {label}", style("✔").green()));
            let output = job.folder.join(job.album.file_name(track, extension));
            let partial = output.with_extension(format!("{extension}.part"));
            let args = encoder_args(job, track, &wav, &partial);
            let handle = thread::spawn(move || encode(&args, &wav, &partial, &output));
            encoders.push((label, handle));
            summary.ripped += 1;
        }
        done_sectors += u64::from(track.audio.sectors);
        bar.set_position(done_sectors);
    }

    bar.set_message(format!("finishing {} encoding…", job.format.label()));
    for (label, handle) in encoders {
        let result = handle
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("encoder thread crashed")));
        if let Err(e) = result {
            bar.println(format!(
                "  {} {label}: encoding failed: {e:#}",
                style("✘").red()
            ));
            summary.failed.push(label);
            summary.ripped -= 1;
        }
    }
    bar.finish_and_clear();
    summary
}

fn rip_track(device: &Path, track: &Track, wav: &Path, bar: &ProgressBar, done: u64) -> Result<()> {
    let mut child = Command::new("cdparanoia")
        .arg("--stderr-progress")
        .arg("--force-cdrom-device")
        .arg(device)
        .arg(track.audio.number.to_string())
        .arg(wav)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't start cdparanoia")?;

    let start = u64::from(track.audio.lba);
    let length = u64::from(track.audio.sectors);
    let mut log: Vec<String> = Vec::new();
    let stderr = BufReader::new(child.stderr.take().expect("stderr is piped"));
    for line in stderr.split(b'\n').map_while(Result::ok) {
        for line in String::from_utf8_lossy(&line).split('\r') {
            // Progress lines look like: "##: -2 [wrote] @ 1234567"
            if let Some(words) = line
                .strip_prefix("##: ")
                .filter(|l| l.contains("[wrote]"))
                .and_then(|l| l.rsplit("@ ").next())
                .and_then(|n| n.trim().parse::<u64>().ok())
            {
                let sector = words / WORDS_PER_SECTOR;
                bar.set_position(done + sector.saturating_sub(start).min(length));
            } else if !line.trim().is_empty() && !line.starts_with("##:") {
                log.push(line.trim().to_string());
            }
        }
    }

    if !child.wait()?.success() || !wav.exists() {
        let tail = log
            .iter()
            .rev()
            .take(3)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        bail!("cdparanoia failed: {tail}");
    }
    Ok(())
}

fn encoder_args(job: &Job, track: &Track, wav: &Path, output: &Path) -> Vec<OsString> {
    let album = job.album;
    let track_no = track.position.to_string();
    let track_total = album.tracks.len().to_string();
    let year = album.year.clone().unwrap_or_default();
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |s: &str| args.push(s.into());

    match job.format {
        Format::Flac => {
            push("flac");
            push("--silent");
            push("--best");
            push("--force");
            let mut tags = vec![
                ("TITLE", track.title.clone()),
                ("ARTIST", track.artist.clone()),
                ("ALBUM", album.title.clone()),
                ("ALBUMARTIST", album.artist.clone()),
                ("TRACKNUMBER", track_no),
                ("TRACKTOTAL", track_total),
                ("DISCNUMBER", album.disc.to_string()),
                ("DISCTOTAL", album.disc_total.to_string()),
                ("DATE", year),
            ];
            if let Some(id) = &album.release_id {
                tags.push(("MUSICBRAINZ_ALBUMID", id.clone()));
            }
            for (key, value) in tags.into_iter().filter(|(_, v)| !v.is_empty()) {
                push(&format!("--tag={key}={value}"));
            }
            if let Some(cover) = job.cover {
                args.push(prefixed("--picture=", cover));
            }
            args.extend(["-o".into(), output.into(), wav.into()]);
        }
        Format::Mp3 => {
            for arg in ["lame", "--quiet", "-V", "0", "--add-id3v2", "--id3v2-utf16"] {
                push(arg);
            }
            let mut tags = vec![
                ("--tt", track.title.clone()),
                ("--ta", track.artist.clone()),
                ("--tl", album.title.clone()),
                ("--tn", format!("{track_no}/{track_total}")),
                ("--tv", format!("TPE2={}", album.artist)),
                ("--tv", format!("TPOS={}/{}", album.disc, album.disc_total)),
            ];
            if !year.is_empty() {
                tags.push(("--ty", year));
            }
            for (flag, value) in tags {
                push(flag);
                push(&value);
            }
            if let Some(cover) = job.cover {
                args.extend(["--ti".into(), cover.into()]);
            }
            args.extend([wav.into(), output.into()]);
        }
        Format::Opus => {
            for arg in ["opusenc", "--quiet", "--bitrate", "160"] {
                push(arg);
            }
            let mut tags = vec![
                ("--title", track.title.clone()),
                ("--artist", track.artist.clone()),
                ("--album", album.title.clone()),
                ("--tracknumber", track_no),
                ("--comment", format!("TRACKTOTAL={track_total}")),
                ("--comment", format!("ALBUMARTIST={}", album.artist)),
                ("--comment", format!("DISCNUMBER={}", album.disc)),
                ("--comment", format!("DISCTOTAL={}", album.disc_total)),
            ];
            if !year.is_empty() {
                tags.push(("--date", year));
            }
            if let Some(id) = &album.release_id {
                tags.push(("--comment", format!("MUSICBRAINZ_ALBUMID={id}")));
            }
            for (flag, value) in tags {
                push(flag);
                push(&value);
            }
            if let Some(cover) = job.cover {
                args.extend(["--picture".into(), cover.into()]);
            }
            args.extend([wav.into(), output.into()]);
        }
    }
    args
}

fn prefixed(prefix: &str, path: &Path) -> OsString {
    let mut arg = OsString::from(prefix);
    arg.push(path);
    arg
}

/// Run the encoder into `<name>.part`, then rename it into place so a half-written
/// file never looks finished. The WAV is deleted afterwards.
fn encode(args: &[OsString], wav: &Path, partial: &Path, output: &Path) -> Result<()> {
    let result = Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("couldn't start {}", args[0].display()));
    let _ = fs::remove_file(wav);
    let result = result?;
    if !result.status.success() {
        let _ = fs::remove_file(partial);
        bail!("{}", String::from_utf8_lossy(&result.stderr).trim());
    }
    fs::rename(partial, output).context("couldn't move the finished file into place")?;
    Ok(())
}
