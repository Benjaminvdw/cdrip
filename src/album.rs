//! What we're about to rip: album info, track titles, and where the files go.

use std::path::{Path, PathBuf};

use crate::discid::{AudioTrack, Toc};
use crate::musicbrainz::{Release, credit_name};

#[derive(Debug, Clone)]
pub struct Album {
    pub artist: String,
    pub title: String,
    pub year: Option<String>,
    pub release_id: Option<String>,
    pub disc: u32,
    pub disc_total: u32,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct Track {
    /// 1-based position on this disc (used for the file name and tags).
    pub position: u32,
    pub title: String,
    pub artist: String,
    pub audio: AudioTrack,
}

impl Album {
    pub fn from_release(release: &Release, toc: &Toc) -> Self {
        let artist = release.artist();
        let medium = release.medium_for(toc);
        let first_number = toc.audio_tracks().first().map_or(1, |t| t.number);
        let tracks = toc
            .audio_tracks()
            .into_iter()
            .map(|audio| {
                let position = u32::from(audio.number - first_number + 1);
                let mb_track =
                    medium.and_then(|m| m.tracks.iter().find(|t| t.position == position));
                Track {
                    position,
                    title: mb_track
                        .map_or_else(|| format!("Track {position:02}"), |t| t.title.clone()),
                    artist: mb_track
                        .and_then(|t| credit_name(&t.artist_credit))
                        .unwrap_or_else(|| artist.clone()),
                    audio,
                }
            })
            .collect();
        Self {
            title: release.title.clone(),
            year: release.year(),
            release_id: Some(release.id.clone()),
            disc: medium.map_or(1, |m| m.position.max(1)),
            disc_total: u32::try_from(release.media.len()).unwrap_or(1).max(1),
            artist,
            tracks,
        }
    }

    /// For discs MusicBrainz doesn't know: generic track names.
    pub fn unknown(toc: &Toc, artist: String, title: String) -> Self {
        let tracks = (1..)
            .zip(toc.audio_tracks())
            .map(|(position, audio)| Track {
                position,
                title: format!("Track {position:02}"),
                artist: artist.clone(),
                audio,
            })
            .collect();
        Self {
            artist,
            title,
            year: None,
            release_id: None,
            disc: 1,
            disc_total: 1,
            tracks,
        }
    }

    /// `<music>/Artist/Album (Year)`
    pub fn folder(&self, music_dir: &Path) -> PathBuf {
        let album = match &self.year {
            Some(year) => format!("{} ({year})", self.title),
            None => self.title.clone(),
        };
        music_dir
            .join(sanitize(&self.artist))
            .join(sanitize(&album))
    }

    /// `01 - Title.flac`, or `2-01 - Title.flac` for disc 2 of a multi-disc album.
    pub fn file_name(&self, track: &Track, extension: &str) -> String {
        let prefix = if self.disc_total > 1 {
            format!("{}-{:02}", self.disc, track.position)
        } else {
            format!("{:02}", track.position)
        };
        sanitize(&format!("{prefix} - {}.{extension}", track.title))
    }
}

/// Make a name safe for file systems, including FAT/exFAT SD cards and phones.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .replace(": ", " - ")
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let mut cleaned = cleaned.trim().trim_end_matches('.').trim_end().to_string();
    // Most file systems allow 255 bytes per name.
    while cleaned.len() > 200 {
        cleaned.pop();
    }
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "_".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discid::TocTrack;

    fn toc() -> Toc {
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
        ];
        Toc::new(tracks, 2000).unwrap()
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize("AC/DC"), "AC_DC");
        assert_eq!(sanitize("Title: Subtitle"), "Title - Subtitle");
        assert_eq!(sanitize("What?"), "What_");
        assert_eq!(sanitize("  Dots...  "), "Dots");
        assert_eq!(sanitize(".."), "_");
        assert_eq!(sanitize("Björk"), "Björk");
        assert!(sanitize(&"é".repeat(300)).len() <= 200);
    }

    #[test]
    fn folder_and_file_names() {
        let mut album = Album::unknown(&toc(), "Artist".into(), "Album".into());
        album.year = Some("1999".into());
        assert_eq!(
            album.folder(Path::new("/music")),
            Path::new("/music/Artist/Album (1999)")
        );
        assert_eq!(
            album.file_name(&album.tracks[1], "flac"),
            "02 - Track 02.flac"
        );
        album.disc = 2;
        album.disc_total = 2;
        assert_eq!(
            album.file_name(&album.tracks[0], "mp3"),
            "2-01 - Track 01.mp3"
        );
    }

    #[test]
    fn maps_musicbrainz_tracks() {
        let json = r#"{"id":"r1","title":"Album","date":"2001-02-03",
            "artist-credit":[{"name":"Band","joinphrase":""}],
            "media":[{"position":1,"track-count":2,"discs":[],"tracks":[
                {"position":1,"title":"Intro"},
                {"position":2,"title":"Song","artist-credit":[{"name":"Band","joinphrase":" feat. "},{"name":"Guest","joinphrase":""}]}
            ]}]}"#;
        let release: Release = serde_json::from_str(json).unwrap();
        let album = Album::from_release(&release, &toc());
        assert_eq!(album.year.as_deref(), Some("2001"));
        assert_eq!(album.tracks[0].title, "Intro");
        assert_eq!(album.tracks[0].artist, "Band");
        assert_eq!(album.tracks[1].artist, "Band feat. Guest");
    }
}
