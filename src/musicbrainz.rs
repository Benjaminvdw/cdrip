//! Looking discs up on MusicBrainz and fetching cover art from the Cover Art Archive.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::discid::Toc;

const USER_AGENT: &str = concat!(
    "cdrip/",
    env!("CARGO_PKG_VERSION"),
    " ( ",
    env!("CARGO_PKG_REPOSITORY"),
    " )"
);

#[derive(Debug, Deserialize)]
struct Lookup {
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub id: String,
    pub title: String,
    pub date: Option<String>,
    pub country: Option<String>,
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<Credit>,
    #[serde(default)]
    pub media: Vec<Medium>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Credit {
    pub name: String,
    #[serde(default)]
    pub joinphrase: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Medium {
    #[serde(default)]
    pub position: u32,
    pub format: Option<String>,
    #[serde(default)]
    pub discs: Vec<Disc>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(rename = "track-count", default)]
    pub track_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Disc {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    #[serde(default)]
    pub position: u32,
    pub title: String,
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<Credit>,
}

/// Join an artist credit like MusicBrainz shows it, e.g. "Simon & Garfunkel".
pub fn credit_name(credit: &[Credit]) -> Option<String> {
    if credit.is_empty() {
        return None;
    }
    Some(
        credit
            .iter()
            .flat_map(|c| [c.name.as_str(), c.joinphrase.as_str()])
            .collect(),
    )
}

impl Release {
    pub fn artist(&self) -> String {
        credit_name(&self.artist_credit).unwrap_or_else(|| "Unknown Artist".into())
    }

    pub fn year(&self) -> Option<String> {
        let date = self.date.as_deref()?;
        let year = date.get(..4)?;
        year.chars()
            .all(|c| c.is_ascii_digit())
            .then(|| year.into())
    }

    /// The medium (disc) of this release that is in the drive.
    pub fn medium_for(&self, toc: &Toc) -> Option<&Medium> {
        let id = toc.disc_id();
        let audio = toc.audio_tracks().len();
        self.media
            .iter()
            .find(|m| m.discs.iter().any(|d| d.id == id))
            .or_else(|| self.media.iter().find(|m| m.track_count == audio))
            .or_else(|| self.media.first())
    }

    /// One line summary to choose between matches.
    pub fn label(&self, toc: &Toc) -> String {
        let mut details: Vec<String> = Vec::new();
        details.extend(self.date.clone().filter(|d| !d.is_empty()));
        details.extend(self.country.clone());
        if let Some(format) = self.medium_for(toc).and_then(|m| m.format.clone()) {
            details.push(format);
        }
        if self.media.len() > 1 {
            details.push(format!("{} discs", self.media.len()));
        }
        format!(
            "{} — {} ({})",
            self.artist(),
            self.title,
            details.join(", ")
        )
    }
}

pub struct Client {
    agent: ureq::Agent,
}

impl Client {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self { agent }
    }

    /// All releases that contain this disc. Empty if MusicBrainz doesn't know it.
    pub fn lookup(&self, toc: &Toc) -> Result<Vec<Release>> {
        let url = format!("https://musicbrainz.org/ws/2/discid/{}", toc.disc_id());
        let response = self
            .agent
            .get(&url)
            .query("toc", toc.to_mb_string())
            .query("inc", "artist-credits recordings")
            .query("cdstubs", "no")
            .query("fmt", "json")
            .call();
        match response {
            Ok(mut response) => {
                let lookup: Lookup = response
                    .body_mut()
                    .read_json()
                    .context("MusicBrainz sent a response we couldn't read")?;
                Ok(lookup.releases)
            }
            Err(ureq::Error::StatusCode(404)) => Ok(Vec::new()),
            Err(e) => Err(e).context("couldn't reach MusicBrainz"),
        }
    }

    /// Front cover (1200px) of a release, if the Cover Art Archive has one.
    pub fn front_cover(&self, release_id: &str) -> Option<Vec<u8>> {
        let url = format!("https://coverartarchive.org/release/{release_id}/front-1200");
        let mut response = self.agent.get(&url).call().ok()?;
        response
            .body_mut()
            .with_config()
            .limit(30 * 1024 * 1024)
            .read_to_vec()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credit(name: &str, join: &str) -> Credit {
        Credit {
            name: name.into(),
            joinphrase: join.into(),
        }
    }

    #[test]
    fn joins_artist_credits() {
        let credits = [credit("Simon", " & "), credit("Garfunkel", "")];
        assert_eq!(credit_name(&credits).unwrap(), "Simon & Garfunkel");
        assert_eq!(credit_name(&[]), None);
    }

    #[test]
    fn parses_lookup_json() {
        let json = r#"{"releases":[{"id":"abc","title":"Hello Nasty","date":"1998-07-14",
            "country":null,"artist-credit":[{"name":"Beastie Boys","joinphrase":""}],
            "media":[{"position":1,"format":"CD","track-count":1,"discs":[{"id":"x"}],
            "tracks":[{"position":1,"title":"Super Disco Breakin'"}]}]}]}"#;
        let lookup: Lookup = serde_json::from_str(json).unwrap();
        let release = &lookup.releases[0];
        assert_eq!(release.artist(), "Beastie Boys");
        assert_eq!(release.year().as_deref(), Some("1998"));
        assert_eq!(release.media[0].tracks[0].title, "Super Disco Breakin'");
    }
}
