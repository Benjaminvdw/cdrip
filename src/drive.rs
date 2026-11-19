//! Talking to the CD drive directly through Linux `ioctl`s (see `linux/cdrom.h`).
//! This is the only module that uses `unsafe`; every call passes a properly sized,
//! `#[repr(C)]` struct (or a plain integer) that the kernel fills in.
#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

use crate::discid::{PREGAP, Toc, TocTrack};

const CDROMREADTOCHDR: libc::c_ulong = 0x5305;
const CDROMREADTOCENTRY: libc::c_ulong = 0x5306;
const CDROMEJECT: libc::c_ulong = 0x5309;
const CDROM_DRIVE_STATUS: libc::c_ulong = 0x5326;
const CDROM_LBA: u8 = 0x01;
const CDROM_LEADOUT: u8 = 0xAA;
const CDROM_DATA_TRACK: u8 = 0x04;
const CDSL_CURRENT: libc::c_int = libc::c_int::MAX;

/// `struct cdrom_tochdr`
#[repr(C)]
#[derive(Default)]
struct TocHeader {
    first_track: u8,
    last_track: u8,
}

/// `struct cdrom_tocentry` (the address union is read as an LBA `int`)
#[repr(C)]
#[derive(Default)]
struct TocEntry {
    track: u8,
    /// Two 4 bit fields: `adr` in the low nibble, `ctrl` in the high nibble.
    adr_ctrl: u8,
    format: u8,
    lba: libc::c_int,
    datamode: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NoDisc,
    TrayOpen,
    NotReady,
    Ready,
    /// The drive doesn't report its status; just try reading it.
    Unknown,
}

fn open(device: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        // O_NONBLOCK lets us open the drive even when it's empty or the tray is open.
        .custom_flags(libc::O_NONBLOCK)
        .open(device)
        .map_err(|e| {
            let hint = match e.kind() {
                ErrorKind::NotFound => {
                    "no CD drive there. Is it plugged in? Pick another one with --device"
                }
                ErrorKind::PermissionDenied => {
                    "permission denied. Add yourself to the `optical` group: sudo usermod -aG optical $USER (then log out and in)"
                }
                _ => "couldn't open the drive",
            };
            anyhow::anyhow!("{}: {hint} ({e})", device.display())
        })
}

fn check(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

pub fn status(device: &Path) -> Result<Status> {
    let drive = open(device)?;
    // SAFETY: CDROM_DRIVE_STATUS takes a plain integer argument and returns the status.
    let ret = unsafe { libc::ioctl(drive.as_raw_fd(), CDROM_DRIVE_STATUS as _, CDSL_CURRENT) };
    Ok(match check(ret) {
        Ok(1) => Status::NoDisc,
        Ok(2) => Status::TrayOpen,
        Ok(3) => Status::NotReady,
        Ok(4) => Status::Ready,
        _ => Status::Unknown,
    })
}

fn read_entry(drive: &File, track: u8) -> io::Result<TocEntry> {
    let mut entry = TocEntry {
        track,
        format: CDROM_LBA,
        ..TocEntry::default()
    };
    // SAFETY: `entry` is a valid `struct cdrom_tocentry` that outlives the call.
    check(unsafe { libc::ioctl(drive.as_raw_fd(), CDROMREADTOCENTRY as _, &raw mut entry) })?;
    Ok(entry)
}

fn to_offset(lba: libc::c_int) -> Result<u32> {
    let lba = u32::try_from(lba).context("drive reported a negative track address")?;
    Ok(lba + PREGAP)
}

/// Read the table of contents of the disc in the drive.
pub fn read_toc(device: &Path) -> Result<Toc> {
    let drive = open(device)?;
    let mut header = TocHeader::default();
    // SAFETY: `header` is a valid `struct cdrom_tochdr` that outlives the call.
    check(unsafe { libc::ioctl(drive.as_raw_fd(), CDROMREADTOCHDR as _, &raw mut header) })
        .context("couldn't read the disc's table of contents")?;

    let mut tracks = Vec::new();
    for number in header.first_track..=header.last_track {
        let entry = read_entry(&drive, number)
            .with_context(|| format!("couldn't read track {number} from the TOC"))?;
        tracks.push(TocTrack {
            number,
            offset: to_offset(entry.lba)?,
            is_audio: (entry.adr_ctrl >> 4) & CDROM_DATA_TRACK == 0,
        });
    }
    let leadout = read_entry(&drive, CDROM_LEADOUT).context("couldn't read the disc's lead-out")?;
    Toc::new(tracks, to_offset(leadout.lba)?)
}

fn spinner(message: String) -> ProgressBar {
    let bar = ProgressBar::new_spinner().with_message(message);
    bar.set_style(
        ProgressStyle::with_template("  {spinner:.magenta} {msg}").expect("valid template"),
    );
    bar.enable_steady_tick(Duration::from_millis(100));
    bar
}

/// Wait until there's a readable disc in the drive and return its TOC.
pub fn wait_for_disc(device: &Path) -> Result<Toc> {
    let mut waiting: Option<ProgressBar> = None;
    let mut failures = 0;
    loop {
        match status(device)? {
            Status::Ready | Status::Unknown => match read_toc(device) {
                Ok(toc) => {
                    if let Some(bar) = waiting {
                        bar.finish_and_clear();
                    }
                    return Ok(toc);
                }
                // Drives often need a few seconds to spin up after the tray closes.
                Err(e) => {
                    failures += 1;
                    if failures >= 10 {
                        return Err(e);
                    }
                }
            },
            Status::NotReady => {
                waiting
                    .get_or_insert_with(|| spinner("Reading the disc…".into()))
                    .set_message("Reading the disc…");
            }
            Status::NoDisc | Status::TrayOpen => {
                waiting.get_or_insert_with(|| {
                    spinner(format!(
                        "Insert an audio CD into {} {}",
                        device.display(),
                        style("(Ctrl+C to quit)").dim()
                    ))
                });
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// Block until the current disc has been taken out (used in --watch mode).
pub fn wait_for_removal(device: &Path) -> Result<()> {
    if status(device)? != Status::Ready {
        return Ok(());
    }
    let bar = spinner("Take the disc out to rip the next one…".into());
    while status(device)? == Status::Ready {
        thread::sleep(Duration::from_secs(1));
    }
    bar.finish_and_clear();
    Ok(())
}

pub fn eject(device: &Path) -> Result<()> {
    let drive = open(device)?;
    // SAFETY: CDROMEJECT takes no argument.
    if check(unsafe { libc::ioctl(drive.as_raw_fd(), CDROMEJECT as _) }).is_err() {
        bail!("couldn't eject {}", device.display());
    }
    Ok(())
}
