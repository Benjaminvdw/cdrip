//! The disc's table of contents (TOC) and the MusicBrainz disc ID derived from it.
//!
//! Spec: <https://musicbrainz.org/doc/Disc_ID_Calculation>

use std::fmt::Write as _;

use anyhow::{Result, bail};
use base64::Engine as _;
use sha1::{Digest, Sha1};

/// Audio CDs start with a 2 second (150 sector) pregap. MusicBrainz offsets include it.
pub const PREGAP: u32 = 150;
pub const SECTORS_PER_SECOND: u32 = 75;
/// Gap between the audio session and a trailing data session on "enhanced" CDs.
const DATA_SESSION_GAP: u32 = 11_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TocTrack {
    pub number: u8,
    /// Start of the track in sectors, MusicBrainz style (LBA + 150).
    pub offset: u32,
    pub is_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toc {
    tracks: Vec<TocTrack>,
    /// End of the last track (MusicBrainz style offset).
    leadout: u32,
}

/// An audio track ready to rip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTrack {
    pub number: u8,
    /// Start sector as the drive (and cdparanoia) count it.
    pub lba: u32,
    pub sectors: u32,
}

impl AudioTrack {
    pub fn seconds(self) -> u32 {
        self.sectors / SECTORS_PER_SECOND
    }
}

impl Toc {
    /// Build a TOC from what the drive reports. A trailing data track (enhanced CD) is
    /// dropped, and the leadout moved back to where the audio session ends, just like
    /// libdiscid does, so the disc ID matches MusicBrainz.
    pub fn new(mut tracks: Vec<TocTrack>, mut leadout: u32) -> Result<Self> {
        if tracks.is_empty() {
            bail!("the disc has no tracks");
        }
        if tracks.len() > 1 && tracks.last().is_some_and(|t| !t.is_audio) {
            let data = tracks.pop().expect("checked above");
            leadout = data.offset.saturating_sub(DATA_SESSION_GAP);
        }
        if !tracks.iter().any(|t| t.is_audio) {
            bail!("this is a data disc, there are no audio tracks on it");
        }
        Ok(Self { tracks, leadout })
    }

    fn first(&self) -> u8 {
        self.tracks[0].number
    }

    fn last(&self) -> u8 {
        self.tracks[self.tracks.len() - 1].number
    }

    pub fn audio_tracks(&self) -> Vec<AudioTrack> {
        self.tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_audio)
            .map(|(i, t)| {
                let end = self.tracks.get(i + 1).map_or(self.leadout, |n| n.offset);
                AudioTrack {
                    number: t.number,
                    lba: t.offset - PREGAP,
                    sectors: end.saturating_sub(t.offset),
                }
            })
            .collect()
    }

    pub fn total_seconds(&self) -> u32 {
        (self.leadout - self.tracks[0].offset) / SECTORS_PER_SECOND
    }

    /// The 28 character MusicBrainz disc ID, e.g. `xUp1F2NkfP8s8jaeFn_Av3jNEI4-`.
    pub fn disc_id(&self) -> String {
        let mut text = String::with_capacity(804);
        let _ = write!(
            text,
            "{:02X}{:02X}{:08X}",
            self.first(),
            self.last(),
            self.leadout
        );
        for number in 1..=99 {
            let offset = self
                .tracks
                .iter()
                .find(|t| t.number == number)
                .map_or(0, |t| t.offset);
            let _ = write!(text, "{offset:08X}");
        }
        let hash = Sha1::digest(text.as_bytes());
        base64::engine::general_purpose::STANDARD
            .encode(hash)
            .chars()
            .map(|c| match c {
                '+' => '.',
                '/' => '_',
                '=' => '-',
                c => c,
            })
            .collect()
    }

    /// The TOC in MusicBrainz' text form: `first last leadout offset1 offset2 ...`.
    /// Used for fuzzy lookups when the exact disc ID isn't known yet.
    pub fn to_mb_string(&self) -> String {
        let mut parts = vec![
            self.first().to_string(),
            self.last().to_string(),
            self.leadout.to_string(),
        ];
        parts.extend(self.tracks.iter().map(|t| t.offset.to_string()));
        parts.join(" ")
    }

    /// Link that lets the user add this disc to MusicBrainz when it isn't known yet.
    pub fn submit_url(&self) -> String {
        format!(
            "https://musicbrainz.org/cdtoc/attach?id={}&tracks={}&toc={}",
            self.disc_id(),
            self.tracks.len(),
            self.to_mb_string().replace(' ', "+")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_toc(offsets: &[u32], leadout: u32) -> Toc {
        let tracks = (1..)
            .zip(offsets)
            .map(|(number, &offset)| TocTrack {
                number,
                offset,
                is_audio: true,
            })
            .collect();
        Toc::new(tracks, leadout).unwrap()
    }

    /// Real disc taken from MusicBrainz:
    /// <https://musicbrainz.org/ws/2/discid/xUp1F2NkfP8s8jaeFn_Av3jNEI4-?fmt=json>
    #[allow(clippy::unreadable_literal)]
    fn known_disc() -> Toc {
        audio_toc(
            &[
                150, 9700, 25887, 39297, 53795, 63735, 77517, 94877, 107270, 123552, 135522,
                148422, 161197, 174790, 192022, 205545, 218010, 228700, 239590, 255470, 266932,
                288750,
            ],
            303_602,
        )
    }

    #[test]
    fn computes_musicbrainz_disc_id() {
        assert_eq!(known_disc().disc_id(), "xUp1F2NkfP8s8jaeFn_Av3jNEI4-");
    }

    #[test]
    fn toc_string() {
        let toc = audio_toc(&[150, 1000], 2000);
        assert_eq!(toc.to_mb_string(), "1 2 2000 150 1000");
    }

    #[test]
    fn audio_track_lengths() {
        let toc = audio_toc(&[150, 1000], 2000);
        let tracks = toc.audio_tracks();
        assert_eq!(
            tracks[0],
            AudioTrack {
                number: 1,
                lba: 0,
                sectors: 850
            }
        );
        assert_eq!(
            tracks[1],
            AudioTrack {
                number: 2,
                lba: 850,
                sectors: 1000
            }
        );
    }

    #[test]
    fn enhanced_cd_drops_trailing_data_track() {
        let tracks = vec![
            TocTrack {
                number: 1,
                offset: 150,
                is_audio: true,
            },
            TocTrack {
                number: 2,
                offset: 1000,
                is_audio: true,
            },
            TocTrack {
                number: 3,
                offset: 20_000,
                is_audio: false,
            },
        ];
        let toc = Toc::new(tracks, 30_000).unwrap();
        assert_eq!(toc.to_mb_string(), "1 2 8600 150 1000");
        assert_eq!(toc.audio_tracks().len(), 2);
    }

    #[test]
    fn data_only_disc_is_rejected() {
        let tracks = vec![TocTrack {
            number: 1,
            offset: 150,
            is_audio: false,
        }];
        assert!(Toc::new(tracks, 5000).is_err());
    }
}
