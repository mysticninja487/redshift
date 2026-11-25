//! Internet radio stations: what a station is, which ones the person has kept, and which ones were
//! played lately (so a station in the queue keeps its name after a restart).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::library::write_json_atomic;

/// How many recently played stations are remembered.
const RECENT_MAX: usize = 40;

/// A radio station, as the directory describes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Station {
    /// The directory's id for it (empty for one typed in by hand).
    pub id: String,
    pub name: String,
    /// The stream address.
    pub url: String,
    pub homepage: String,
    pub country: String,
    pub tags: String,
    /// "MP3", "AAC", "FLAC", "OGG" and so on, as the directory lists it.
    pub codec: String,
    /// What the directory lists, in kbit/s; 0 when unknown.
    pub bitrate: u32,
    /// A lossless codec: the audio is what the station sent, with nothing thrown away.
    pub lossless: bool,
    /// The address is https.
    pub secure: bool,
    /// Redshift can play this kind of stream (HLS and HE-AAC can't be yet).
    pub supported: bool,
}

impl Station {
    /// From a bare address: the name is the host until the station says otherwise.
    pub fn from_url(url: &str) -> Station {
        let uri: Option<ureq::http::Uri> = url.trim().parse().ok();
        let host = uri.as_ref().and_then(|u| u.host()).unwrap_or(url).to_string();
        Station {
            name: host,
            url: url.trim().to_string(),
            secure: url.trim().to_ascii_lowercase().starts_with("https://"),
            supported: true,
            ..Station::default()
        }
    }
}

/// Whether a directory codec name is lossless.
pub fn codec_is_lossless(codec: &str) -> bool {
    let c = codec.to_ascii_lowercase();
    c.contains("flac") || c.contains("alac") || c.contains("wav") || c.contains("pcm")
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    favourites: Vec<Station>,
    recent: Vec<Station>,
}

/// Kept stations, saved to `stations.json`.
pub struct StationStore {
    path: PathBuf,
    saved: Saved,
}

impl StationStore {
    pub fn open(path: PathBuf) -> Result<StationStore> {
        let saved = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                let mut bad = path.as_os_str().to_owned();
                bad.push(".bad");
                let _ = fs::rename(&path, PathBuf::from(bad));
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        Ok(StationStore { path, saved })
    }

    fn save(&self) -> Result<()> {
        write_json_atomic(&self.path, &self.saved)
    }

    pub fn favourites(&self) -> Vec<Station> {
        self.saved.favourites.clone()
    }

    pub fn recent(&self) -> Vec<Station> {
        self.saved.recent.clone()
    }

    pub fn is_favourite(&self, url: &str) -> bool {
        self.saved.favourites.iter().any(|s| s.url == url)
    }

    /// Station details for an address, from the favourites first, then the recently played.
    pub fn find(&self, url: &str) -> Option<&Station> {
        self.saved.favourites.iter().chain(self.saved.recent.iter()).find(|s| s.url == url)
    }

    pub fn set_favourite(&mut self, station: &Station, on: bool) -> Result<()> {
        self.saved.favourites.retain(|s| s.url != station.url);
        if on {
            self.saved.favourites.insert(0, station.clone());
        }
        self.save()
    }

    /// Notes that a station was played: newest first, no repeats, the oldest forgotten.
    pub fn remember(&mut self, station: &Station) -> Result<()> {
        if self.saved.recent.first().is_some_and(|s| s == station) {
            return Ok(());
        }
        self.saved.recent.retain(|s| s.url != station.url);
        self.saved.recent.insert(0, station.clone());
        self.saved.recent.truncate(RECENT_MAX);
        // A favourite keeps its details fresh too.
        if let Some(f) = self.saved.favourites.iter_mut().find(|s| s.url == station.url) {
            *f = station.clone();
        }
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(name: &str, url: &str) -> Station {
        Station { name: name.into(), url: url.into(), supported: true, ..Station::default() }
    }

    #[test]
    fn favourites_and_recent_survive_a_restart() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("stations.json");
        {
            let mut s = StationStore::open(file.clone()).unwrap();
            s.set_favourite(&st("Jazz", "https://j.example/stream"), true).unwrap();
            s.remember(&st("Rock", "http://r.example/live")).unwrap();
        }
        let s = StationStore::open(file).unwrap();
        assert!(s.is_favourite("https://j.example/stream"));
        assert_eq!(s.find("http://r.example/live").unwrap().name, "Rock");
        assert!(s.find("http://nobody.example/").is_none());
    }

    #[test]
    fn recent_is_newest_first_without_repeats_and_bounded() {
        let d = tempfile::tempdir().unwrap();
        let mut s = StationStore::open(d.path().join("s.json")).unwrap();
        for i in 0..(RECENT_MAX + 5) {
            s.remember(&st(&format!("S{i}"), &format!("http://h/{i}"))).unwrap();
        }
        s.remember(&st("S10", "http://h/10")).unwrap();
        let r = s.recent();
        assert_eq!(r.len(), RECENT_MAX);
        assert_eq!(r[0].url, "http://h/10");
        assert_eq!(r.iter().filter(|x| x.url == "http://h/10").count(), 1);
    }

    #[test]
    fn a_bare_address_gets_its_host_as_a_name() {
        let s = Station::from_url(" https://stream.example.org:8000/live.flac ");
        assert_eq!((s.name.as_str(), s.secure), ("stream.example.org", true));
        assert_eq!(s.url, "https://stream.example.org:8000/live.flac");
    }

    #[test]
    fn lossless_codecs_are_recognised() {
        assert!(codec_is_lossless("FLAC") && codec_is_lossless("FLAC,OGG") && codec_is_lossless("ALAC"));
        assert!(!codec_is_lossless("MP3") && !codec_is_lossless("AAC+") && !codec_is_lossless("OGG"));
    }
}
