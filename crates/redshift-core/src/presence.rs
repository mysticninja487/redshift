//! Discord rich presence: "Listening to <song>" on the person's Discord profile, with a card that
//! can be shaped line by line (what the lines say, the cover art, the progress bar, links and buttons).
//!
//! Redshift talks to the Discord app running on the same computer through its local pipe; Discord
//! does the rest, so this module never touches the network itself. The only online part is the
//! optional cover-art lookup ([`crate::net::artwork`]), which goes through the usual online switch.
//! It is off until switched on. Everything here is plain `std`: the framing is eight bytes
//! (opcode, length) followed by JSON.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::net::encode_query;

/// The Discord application this build shows itself as. Empty until one is registered at
/// discord.com/developers: the name given there is what Discord shows after "Listening to", and
/// its icon is the fallback picture. The person can also paste their own in Settings.
pub const DEFAULT_CLIENT_ID: &str = "1573295781140496395";

/// The asset key (uploaded to the Discord application) of the Redshift logo: the card's picture when
/// there is no cover art, and the little badge over the cover art when there is.
pub const LOGO_ASSET: &str = "redshift";
const PROJECT_URL: &str = "https://github.com/mysticninja487/redshift";
const APP_NAME: &str = "Redshift";

/// Discord drops updates that come faster than this, and allows only a handful per window.
const SEND_FLOOR: Duration = Duration::from_millis(1500);
const FRAME_WINDOW: Duration = Duration::from_secs(21);
const MAX_FRAMES: usize = 5;
/// An unchanged card is sent again this often, which puts it back if Discord lost it.
const REFRESH: Duration = Duration::from_secs(45);
const RETRY_NOT_RUNNING: Duration = Duration::from_secs(15);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(5);
/// A song's start moves by a second or two from rounding; more than this is a seek.
const SEEK_SLACK_MS: i64 = 3000;
const MAX_FIELD: usize = 128;
const MAX_ASSET_URL: usize = 256;
const MAX_BUTTON_URL: usize = 512;
const MAX_BUTTON_LABEL: usize = 32;

/// How the card is put together. Every choice is a short word, so a settings file from another
/// version (or one edited by hand) never stops it loading: an unknown word means the default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenceSettings {
    pub enabled: bool,
    /// Which line Discord writes after "Listening to": `app` | `line1` | `line2`.
    pub status_line: String,
    /// Replaces "Redshift" after "Listening to". Empty keeps the application's own name.
    pub app_name: String,
    /// Card line 1: `title` | `artist` | `album`.
    pub line1: String,
    /// Card line 2: `title` | `artist` | `album` | `artist_album` | `off`.
    pub line2: String,
    /// Show the album's cover art (found online when Go online is on; the Redshift logo otherwise).
    pub cover: bool,
    /// Card line 3 (also the cover's hover text): `album` | `title` | `artist` | `off`.
    pub line3: String,
    /// Make line 1, line 2 and the cover open a search for what they show.
    pub link_line1: bool,
    pub link_line2: bool,
    pub link_cover: bool,
    /// Where those links and the buttons search: `youtube_music` | `spotify` | `apple_music` | `youtube` | `lastfm`.
    pub link_provider: String,
    /// Show the elapsed / remaining bar (never while paused: Discord would keep it running).
    pub timestamps: bool,
    /// Put the Redshift logo in the corner of the cover art.
    pub badge: bool,
    /// Keep the card up while paused, once something has played this session.
    pub show_paused: bool,
    /// Say only that music is playing: no title, artist, album, cover, bar, links or buttons.
    pub hide_details: bool,
    /// The two buttons: `listen` | `album` | `artist` | `app` | `off`.
    pub button1: String,
    pub button2: String,
    /// A Discord application id of your own; empty uses [`DEFAULT_CLIENT_ID`].
    pub client_id: String,
}

impl Default for PresenceSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            status_line: "line2".into(),
            app_name: String::new(),
            line1: "title".into(),
            line2: "artist".into(),
            cover: true,
            line3: "album".into(),
            link_line1: false,
            link_line2: false,
            link_cover: false,
            link_provider: "youtube_music".into(),
            timestamps: true,
            badge: true,
            show_paused: false,
            hide_details: false,
            button1: "listen".into(),
            button2: "app".into(),
            client_id: String::new(),
        }
    }
}

impl PresenceSettings {
    /// The id to connect with: the person's own when it looks like one, else the built-in.
    pub fn effective_client_id(&self) -> Option<&str> {
        let own = self.client_id.trim();
        if valid_client_id(own) {
            return Some(own);
        }
        valid_client_id(DEFAULT_CLIENT_ID).then_some(DEFAULT_CLIENT_ID)
    }

    /// Whether the card wants a cover picture looked up for the song.
    pub fn wants_cover(&self) -> bool {
        self.cover && !self.hide_details
    }
}

/// Discord ids are numbers of 17 to 20 digits; be a little looser than that.
pub fn valid_client_id(id: &str) -> bool {
    (15..=25).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

/// What the app is doing, in words for the settings page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresenceStatus {
    /// `off`, `no_id`, `waiting` (Discord is not running), `connected` or `error`.
    pub state: &'static str,
    pub detail: String,
}

impl PresenceStatus {
    fn new(state: &'static str, detail: impl Into<String>) -> Self {
        Self { state, detail: detail.into() }
    }
}

// ---- the card ----

/// The song being played, as far as the card is concerned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: Option<f64>,
    pub position_secs: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Button {
    pub label: String,
    pub url: String,
}

/// One activity as Discord shows it. Times are Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Card {
    /// Replaces the application's name after "Listening to".
    pub name: Option<String>,
    /// 0 = the name, 1 = line 2, 2 = line 1 (Discord's `status_display_type`).
    pub status_type: u8,
    pub details: String,
    pub details_url: Option<String>,
    pub state: Option<String>,
    pub state_url: Option<String>,
    /// An asset key or an https picture address.
    pub large_image: String,
    pub large_text: Option<String>,
    pub large_url: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub buttons: Vec<Button>,
}

/// Discord wants 2 to 128 characters in every text field.
fn clip(s: &str) -> String {
    let s = s.trim();
    let mut out: String = s.chars().take(MAX_FIELD - 1).collect();
    if s.chars().count() > MAX_FIELD - 1 {
        out.pop();
        out.push('…');
    }
    while out.chars().count() < 2 {
        out.push('\u{a0}');
    }
    out
}

fn nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// A choice from `allowed`, or `default` for anything else.
fn pick(v: &str, allowed: &[&'static str], default: &'static str) -> &'static str {
    allowed.iter().copied().find(|a| *a == v).unwrap_or(default)
}

fn text_for(slot: &str, t: &TrackInfo) -> Option<String> {
    match slot {
        "title" => nonempty(&t.title),
        "artist" => nonempty(&t.artist),
        "album" => nonempty(&t.album),
        "artist_album" => match (nonempty(&t.artist), nonempty(&t.album)) {
            (Some(a), Some(b)) => Some(format!("{a} — {b}")),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        },
        _ => None,
    }
}

fn provider_name(p: &str) -> &'static str {
    match p {
        "spotify" => "Spotify",
        "apple_music" => "Apple Music",
        "youtube" => "YouTube",
        "lastfm" => "Last.fm",
        _ => "YouTube Music",
    }
}

/// A search for `q` on the chosen service. Nothing is known about the song there: it is only a search.
pub fn search_url(provider: &str, q: &str) -> String {
    let e = encode_query(q);
    match provider {
        "spotify" => format!("https://open.spotify.com/search/{e}"),
        "apple_music" => format!("https://music.apple.com/search?term={e}"),
        "youtube" => format!("https://www.youtube.com/results?search_query={e}"),
        "lastfm" => format!("https://www.last.fm/search?q={e}"),
        _ => format!("https://music.youtube.com/search?q={e}"),
    }
}

/// Where a slot's link goes: what it shows decides what is searched for.
fn link_for(slot: &str, provider: &str, t: &TrackInfo) -> Option<String> {
    let artist = t.artist.trim();
    let q = match slot {
        "title" => format!("{artist} {}", t.title.trim()),
        "artist" => artist.to_string(),
        "album" => format!("{artist} {}", t.album.trim()),
        "artist_album" => format!("{artist} {}", t.album.trim()),
        _ => return None,
    };
    let q = q.trim();
    if q.is_empty() || (matches!(slot, "album" | "artist_album") && t.album.trim().is_empty() && artist.is_empty()) {
        return None;
    }
    let url = search_url(provider, q);
    (url.len() <= MAX_BUTTON_URL).then_some(url)
}

fn button_for(kind: &str, provider: &str, t: &TrackInfo) -> Option<Button> {
    let (label, url) = match kind {
        "listen" => (format!("Listen on {}", provider_name(provider)), link_for("title", provider, t)?),
        "album" if !t.album.trim().is_empty() => (format!("Album on {}", provider_name(provider)), link_for("album", provider, t)?),
        "artist" if !t.artist.trim().is_empty() => (format!("Artist on {}", provider_name(provider)), link_for("artist", provider, t)?),
        "app" => ("Get Redshift".to_string(), PROJECT_URL.to_string()),
        _ => return None,
    };
    Some(Button { label: label.chars().take(MAX_BUTTON_LABEL).collect(), url })
}

/// The card for `track` under `s`. `playing` is false for a paused song (no bar then). `cover` is the
/// address of the song's cover picture, when one was found.
pub fn build_card(s: &PresenceSettings, track: &TrackInfo, playing: bool, cover: Option<&str>, now_ms: i64) -> Card {
    let name = nonempty(&s.app_name).map(|n| clip(&n));
    let mut c = Card { name, large_image: LOGO_ASSET.into(), ..Card::default() };
    if s.hide_details {
        c.details = "Listening to music".into();
        c.status_type = 0;
        if playing {
            c.start = Some(now_ms - (track.position_secs.max(0.0) * 1000.0) as i64);
        }
        return c;
    }
    let provider = pick(&s.link_provider, &["youtube_music", "spotify", "apple_music", "youtube", "lastfm"], "youtube_music");
    let slot1 = pick(&s.line1, &["title", "artist", "album"], "title");
    let slot2 = pick(&s.line2, &["title", "artist", "album", "artist_album", "off"], "artist");
    let slot3 = pick(&s.line3, &["album", "title", "artist", "off"], "album");

    // Line 1 is the one line Discord will not draw a card without, so a song that lacks it (a single
    // with no album) shows its title there instead, and its link follows.
    let line1 = text_for(slot1, track);
    let used1 = if line1.is_some() { slot1 } else { "title" };
    c.details = clip(&line1.or_else(|| nonempty(&track.title)).unwrap_or_else(|| "Unknown song".into()));
    c.status_type = match s.status_line.as_str() {
        "app" => 0,
        "line1" => 2,
        _ => 1,
    };
    if s.link_line1 {
        c.details_url = link_for(used1, provider, track);
    }
    if let Some(t2) = text_for(slot2, track) {
        c.state = Some(clip(&t2));
        if s.link_line2 {
            c.state_url = link_for(slot2, provider, track);
        }
    }
    if playing && s.timestamps {
        let start = now_ms - (track.position_secs.max(0.0) * 1000.0) as i64;
        c.start = Some(start);
        c.end = track.duration_secs.filter(|d| d.is_finite() && *d > 0.0).map(|d| start + (d * 1000.0) as i64);
    }
    if let Some(url) = cover.filter(|u| s.cover && u.starts_with("https://") && u.len() <= MAX_ASSET_URL) {
        c.large_image = url.to_string();
        c.large_text = text_for(slot3, track).map(|t| clip(&t));
        if s.link_cover {
            c.large_url = link_for("album", provider, track).or_else(|| link_for("title", provider, track));
        }
        if s.badge {
            c.small_image = Some(LOGO_ASSET.into());
            c.small_text = Some(clip(c.name.as_deref().unwrap_or(APP_NAME)));
        }
    } else if s.cover {
        // No cover found: the logo stands in, and line 3 still has somewhere to be.
        c.large_text = text_for(slot3, track).map(|t| clip(&t));
    }
    c.buttons = [&s.button1, &s.button2].iter().filter_map(|k| button_for(k, provider, track)).take(2).collect();
    c
}

impl Card {
    fn to_json(&self) -> Value {
        // Type 2 is "Listening to".
        let mut a = json!({ "type": 2, "details": self.details, "status_display_type": self.status_type });
        if let Some(n) = &self.name {
            a["name"] = json!(n);
        }
        if let Some(u) = &self.details_url {
            a["details_url"] = json!(u);
        }
        if let Some(s) = &self.state {
            a["state"] = json!(s);
        }
        if let Some(u) = &self.state_url {
            a["state_url"] = json!(u);
        }
        let mut assets = json!({ "large_image": self.large_image });
        if let Some(t) = &self.large_text {
            assets["large_text"] = json!(t);
        }
        if let Some(u) = &self.large_url {
            assets["large_url"] = json!(u);
        }
        if let Some(i) = &self.small_image {
            assets["small_image"] = json!(i);
        }
        if let Some(t) = &self.small_text {
            assets["small_text"] = json!(t);
        }
        a["assets"] = assets;
        if let Some(start) = self.start {
            a["timestamps"] = json!({ "start": start });
            if let Some(end) = self.end {
                a["timestamps"]["end"] = json!(end);
            }
        }
        if !self.buttons.is_empty() {
            a["buttons"] = json!(self.buttons.iter().map(|b| json!({ "label": b.label, "url": b.url })).collect::<Vec<_>>());
        }
        a
    }

    /// The same card as far as Discord shows it: the start may differ by rounding, not by a seek.
    fn same_as(&self, o: &Card) -> bool {
        let time_ok = match (self.start, o.start) {
            (None, None) => true,
            (Some(a), Some(b)) => (a - b).abs() <= SEEK_SLACK_MS,
            _ => false,
        };
        time_ok && self.end.is_some() == o.end.is_some() && Card { start: None, end: None, ..self.clone() } == Card { start: None, end: None, ..o.clone() }
    }
}

// ---- the wire ----

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

pub fn write_frame<W: Write>(w: &mut W, op: u32, body: &str) -> io::Result<()> {
    let mut buf = Vec::with_capacity(8 + body.len());
    buf.extend_from_slice(&op.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(body.as_bytes());
    w.write_all(&buf)?;
    w.flush()
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > 1 << 16 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Discord sent an oversized message"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

fn other(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

fn message_of(v: &Value) -> String {
    v["data"]["message"].as_str().or_else(|| v["message"].as_str()).unwrap_or("no reason given").to_string()
}

/// A connection to Discord that has been through the handshake.
pub struct Client<S> {
    io: S,
    nonce: u64,
}

impl<S: Read + Write> Client<S> {
    pub fn handshake(mut io: S, client_id: &str) -> io::Result<Self> {
        write_frame(&mut io, OP_HANDSHAKE, &json!({ "v": 1, "client_id": client_id }).to_string())?;
        loop {
            let (op, v) = read_frame(&mut io)?;
            match op {
                OP_CLOSE => return Err(other(format!("Discord closed the connection: {}", message_of(&v)))),
                OP_PING => write_frame(&mut io, OP_PONG, &v.to_string())?,
                OP_FRAME if v["evt"] == "READY" => return Ok(Self { io, nonce: 0 }),
                OP_FRAME if v["evt"] == "ERROR" => return Err(other(format!("Discord refused the application id: {}", message_of(&v)))),
                _ => {}
            }
        }
    }

    /// Shows `card`, or clears the profile line when `None`.
    pub fn set_activity(&mut self, card: Option<&Card>) -> io::Result<()> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        let mut args = json!({ "pid": std::process::id() });
        if let Some(a) = card {
            args["activity"] = a.to_json();
        }
        write_frame(&mut self.io, OP_FRAME, &json!({ "cmd": "SET_ACTIVITY", "args": args, "nonce": nonce }).to_string())?;
        loop {
            let (op, v) = read_frame(&mut self.io)?;
            match op {
                OP_CLOSE => return Err(other(format!("Discord closed the connection: {}", message_of(&v)))),
                OP_PING => write_frame(&mut self.io, OP_PONG, &v.to_string())?,
                OP_FRAME if v["nonce"] == nonce.as_str() => {
                    return if v["evt"] == "ERROR" { Err(other(message_of(&v))) } else { Ok(()) };
                }
                _ => {}
            }
        }
    }
}

// ---- finding Discord ----

pub trait Pipe: Read + Write + Send {}
impl<T: Read + Write + Send> Pipe for T {}
pub type Stream = Box<dyn Pipe>;

/// Opens Discord's local pipe. It is `discord-ipc-0` to `-9`, one per running Discord.
pub fn connect_discord() -> io::Result<Stream> {
    #[cfg(windows)]
    {
        for i in 0..10 {
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")) {
                return Ok(Box::new(f));
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "Discord is not running"))
    }
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        for var in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
            if let Some(d) = std::env::var_os(var) {
                dirs.push(d.into());
            }
        }
        dirs.push("/tmp".into());
        let mut with_sub = Vec::new();
        for d in &dirs {
            with_sub.push(d.join("app/com.discordapp.Discord"));
            with_sub.push(d.join("snap.discord"));
        }
        dirs.extend(with_sub);
        for dir in &dirs {
            for i in 0..10 {
                if let Ok(s) = UnixStream::connect(dir.join(format!("discord-ipc-{i}"))) {
                    s.set_read_timeout(Some(Duration::from_secs(3))).ok();
                    s.set_write_timeout(Some(Duration::from_secs(3))).ok();
                    return Ok(Box::new(s));
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "Discord is not running"))
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Discord presence is not available here"))
    }
}


// ---- keeping the profile line right ----

/// What should be on the profile right now. `entry` tells songs apart.
pub struct Want {
    pub entry: u64,
    pub card: Card,
}

type Connector = Box<dyn Fn() -> io::Result<Stream> + Send>;

pub struct Runner {
    connector: Connector,
    conn: Option<Client<Stream>>,
    conn_id: String,
    /// What Discord was last told.
    last: Option<(u64, Card)>,
    last_sent: Option<Instant>,
    /// When the last few updates went out, to stay inside what Discord allows per window.
    sent_at: VecDeque<Instant>,
    retry_at: Option<Instant>,
    status: PresenceStatus,
}

impl Default for Runner {
    fn default() -> Self {
        Self::with_connector(Box::new(connect_discord))
    }
}

impl Runner {
    pub fn with_connector(connector: Connector) -> Self {
        Self { connector, conn: None, conn_id: String::new(), last: None, last_sent: None, sent_at: VecDeque::new(), retry_at: None, status: PresenceStatus::new("off", "") }
    }

    pub fn status(&self) -> PresenceStatus {
        self.status.clone()
    }

    fn drop_conn(&mut self) {
        self.conn = None;
        self.last = None;
        self.last_sent = None;
        self.sent_at.clear();
    }

    /// Takes the line off the profile (if it was there) and lets go of Discord.
    fn shut_down(&mut self) {
        if let (Some(c), Some(_)) = (self.conn.as_mut(), self.last.as_ref()) {
            let _ = c.set_activity(None);
        }
        self.drop_conn();
        self.retry_at = None;
    }

    /// Whether another update may go out now: not too soon after the last, and not more than
    /// [`MAX_FRAMES`] inside [`FRAME_WINDOW`].
    fn may_send(&mut self, now: Instant) -> bool {
        while self.sent_at.front().is_some_and(|t| now.duration_since(*t) > FRAME_WINDOW) {
            self.sent_at.pop_front();
        }
        if self.last_sent.is_some_and(|t| now.duration_since(t) < SEND_FLOOR) {
            return false;
        }
        self.sent_at.len() < MAX_FRAMES
    }

    /// Call about once a second with what should be shown (`None` for nothing to show).
    pub fn tick(&mut self, settings: &PresenceSettings, want: Option<Want>, now: Instant) {
        if !settings.enabled {
            self.shut_down();
            self.status = PresenceStatus::new("off", "");
            return;
        }
        let Some(id) = settings.effective_client_id() else {
            self.shut_down();
            self.status = PresenceStatus::new("no_id", "");
            return;
        };
        if self.conn.is_some() && self.conn_id != id {
            self.shut_down();
        }
        if self.conn.is_none() {
            if self.retry_at.is_some_and(|t| now < t) {
                return;
            }
            match (self.connector)().and_then(|s| Client::handshake(s, id)) {
                Ok(c) => {
                    self.conn = Some(c);
                    self.conn_id = id.to_string();
                    self.last = None;
                    self.retry_at = None;
                    self.status = PresenceStatus::new("connected", "");
                }
                Err(e) => {
                    let missing = matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::PermissionDenied);
                    self.retry_at = Some(now + if missing { RETRY_NOT_RUNNING } else { RETRY_AFTER_ERROR });
                    self.status = if missing { PresenceStatus::new("waiting", "") } else { PresenceStatus::new("error", e.to_string()) };
                    return;
                }
            }
        }
        let same = match (&self.last, &want) {
            (None, None) => true,
            (Some((id, a)), Some(w)) => *id == w.entry && a.same_as(&w.card),
            _ => false,
        };
        let stale = same && want.is_some() && self.last_sent.is_some_and(|t| now.duration_since(t) >= REFRESH);
        if (same && !stale) || !self.may_send(now) {
            return;
        }
        let Some(conn) = self.conn.as_mut() else { return };
        match conn.set_activity(want.as_ref().map(|w| &w.card)) {
            Ok(()) => {
                self.last = want.map(|w| (w.entry, w.card));
                self.last_sent = Some(now);
                self.sent_at.push_back(now);
                self.status = PresenceStatus::new("connected", "");
            }
            Err(e) => {
                self.drop_conn();
                self.retry_at = Some(now + RETRY_AFTER_ERROR);
                self.status = PresenceStatus::new("error", e.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn track() -> TrackInfo {
        TrackInfo { title: "Carrier Wave".into(), artist: "Paper Satellites".into(), album: "Signal & Noise".into(), duration_secs: Some(200.4), position_secs: 30.7 }
    }

    fn card(s: &PresenceSettings) -> Card {
        build_card(s, &track(), true, None, NOW)
    }

    fn song(entry: u64, start: i64) -> Option<Want> {
        Some(Want { entry, card: Card { details: format!("Song {entry}"), state: Some("Artist".into()), large_image: LOGO_ASSET.into(), start: Some(start), end: Some(start + 200_000), ..Card::default() } })
    }

    #[test]
    fn client_ids_are_checked() {
        assert!(valid_client_id("123456789012345678"));
        assert!(!valid_client_id(""));
        assert!(!valid_client_id("12345"));
        assert!(!valid_client_id("12345678901234567x"));
        let mut s = PresenceSettings::default();
        assert!(valid_client_id(DEFAULT_CLIENT_ID), "the built-in application id must look like one");
        assert_eq!(s.effective_client_id(), Some(DEFAULT_CLIENT_ID), "nothing typed: the built-in one");
        s.client_id = " 123456789012345678 ".into();
        assert_eq!(s.effective_client_id(), Some("123456789012345678"), "the person's own wins");
        s.client_id = "not an id".into();
        assert_eq!(s.effective_client_id(), Some(DEFAULT_CLIENT_ID), "something that is not an id is ignored");
    }

    #[test]
    fn it_is_off_by_default_and_settings_from_any_version_load() {
        let d = PresenceSettings::default();
        assert!(!d.enabled);
        assert_eq!(serde_json::from_str::<PresenceSettings>("{}").unwrap(), d);
        // the 0.6.2 shape, and a word this version has never heard of
        let old: PresenceSettings = serde_json::from_str(r#"{"enabled":true,"share":"generic","client_id":"123456789012345678"}"#).unwrap();
        assert!(old.enabled && old.line1 == "title" && old.client_id == "123456789012345678");
        let odd: PresenceSettings = serde_json::from_str(r#"{"line1":"banana","line2":"???","button1":"nope","link_provider":"gopher"}"#).unwrap();
        let c = card(&odd);
        assert_eq!(c.details, "Carrier Wave", "an unknown word is the default");
        assert_eq!(c.state.as_deref(), Some("Paper Satellites"));
        assert_eq!(c.buttons.len(), 1, "an unknown button is no button; the second is still the app");
    }

    #[test]
    fn the_default_card_has_title_artist_album_the_bar_and_two_buttons() {
        let c = card(&PresenceSettings::default());
        assert_eq!(c.details, "Carrier Wave");
        assert_eq!(c.state.as_deref(), Some("Paper Satellites"));
        assert_eq!(c.status_type, 1);
        assert_eq!(c.start, Some(NOW - 30_700));
        assert_eq!(c.end, Some(NOW - 30_700 + 200_400));
        assert_eq!(c.large_image, LOGO_ASSET, "no cover found: the logo");
        assert_eq!(c.large_text.as_deref(), Some("Signal & Noise"));
        assert!(c.small_image.is_none(), "a badge only goes over a real cover");
        assert_eq!(c.buttons.len(), 2);
        assert_eq!(c.buttons[0].label, "Listen on YouTube Music");
        assert_eq!(c.buttons[0].url, "https://music.youtube.com/search?q=Paper%20Satellites%20Carrier%20Wave");
        assert_eq!(c.buttons[1], Button { label: "Get Redshift".into(), url: PROJECT_URL.into() });
        let j = c.to_json();
        assert_eq!(j["type"], 2);
        assert_eq!(j["status_display_type"], 1);
        assert_eq!(j["assets"]["large_image"], "redshift");
        assert_eq!(j["timestamps"]["start"], NOW - 30_700);
        assert!(j.get("details_url").is_none(), "links are opt-in");
    }

    #[test]
    fn a_found_cover_replaces_the_logo_and_the_logo_becomes_the_badge() {
        let url = "https://is1-ssl.mzstatic.com/image/thumb/x/600x600bb.jpg";
        let c = build_card(&PresenceSettings::default(), &track(), true, Some(url), NOW);
        assert_eq!(c.large_image, url);
        assert_eq!(c.small_image.as_deref(), Some(LOGO_ASSET));
        assert_eq!(c.small_text.as_deref(), Some("Redshift"));
        let no_badge = build_card(&PresenceSettings { badge: false, ..PresenceSettings::default() }, &track(), true, Some(url), NOW);
        assert!(no_badge.small_image.is_none());
        let no_cover = build_card(&PresenceSettings { cover: false, ..PresenceSettings::default() }, &track(), true, Some(url), NOW);
        assert_eq!(no_cover.large_image, LOGO_ASSET);
        assert!(no_cover.large_text.is_none() && no_cover.small_image.is_none());
        let insecure = build_card(&PresenceSettings::default(), &track(), true, Some("http://example.com/a.jpg"), NOW);
        assert_eq!(insecure.large_image, LOGO_ASSET, "only https pictures");
    }

    #[test]
    fn lines_status_and_name_follow_the_settings() {
        let s = PresenceSettings { status_line: "line1".into(), line1: "artist".into(), line2: "artist_album".into(), app_name: "My Player".into(), ..PresenceSettings::default() };
        let c = card(&s);
        assert_eq!(c.details, "Paper Satellites");
        assert_eq!(c.state.as_deref(), Some("Paper Satellites — Signal & Noise"));
        assert_eq!(c.status_type, 2);
        assert_eq!(c.name.as_deref(), Some("My Player"));
        assert_eq!(c.to_json()["name"], "My Player");
        assert_eq!(card(&PresenceSettings { status_line: "app".into(), ..PresenceSettings::default() }).status_type, 0);
        assert!(card(&PresenceSettings { line2: "off".into(), ..PresenceSettings::default() }).state.is_none());
        // a song with no album: line 1 falls back to the title rather than leaving the card empty
        let single = TrackInfo { album: String::new(), ..track() };
        let c = build_card(&PresenceSettings { line1: "album".into(), ..PresenceSettings::default() }, &single, true, None, NOW);
        assert_eq!(c.details, "Carrier Wave");
    }

    #[test]
    fn links_and_buttons_search_the_chosen_service_for_what_the_line_shows() {
        let s = PresenceSettings { link_line1: true, link_line2: true, link_cover: true, link_provider: "spotify".into(), line2: "album".into(), button1: "artist".into(), button2: "album".into(), ..PresenceSettings::default() };
        let c = build_card(&s, &track(), true, Some("https://a.mzstatic.com/x.jpg"), NOW);
        assert_eq!(c.details_url.as_deref(), Some("https://open.spotify.com/search/Paper%20Satellites%20Carrier%20Wave"));
        assert_eq!(c.state_url.as_deref(), Some("https://open.spotify.com/search/Paper%20Satellites%20Signal%20%26%20Noise"));
        assert_eq!(c.large_url, c.state_url, "the cover stands for the album");
        assert_eq!(c.buttons[0].label, "Artist on Spotify");
        assert_eq!(c.buttons[1].label, "Album on Spotify");
        assert_eq!(search_url("apple_music", "a b"), "https://music.apple.com/search?term=a%20b");
        assert_eq!(search_url("youtube", "a"), "https://www.youtube.com/results?search_query=a");
        assert_eq!(search_url("lastfm", "a"), "https://www.last.fm/search?q=a");
        // no album, no album button
        let single = TrackInfo { album: String::new(), ..track() };
        assert!(build_card(&s, &single, true, None, NOW).buttons.iter().all(|b| !b.label.starts_with("Album")));
    }

    #[test]
    fn a_paused_card_has_no_bar_and_hiding_details_says_nothing_about_the_song() {
        let paused = build_card(&PresenceSettings::default(), &track(), false, None, NOW);
        assert!(paused.start.is_none() && paused.end.is_none());
        assert!(paused.to_json().get("timestamps").is_none());
        let no_bar = card(&PresenceSettings { timestamps: false, ..PresenceSettings::default() });
        assert!(no_bar.start.is_none());

        let s = PresenceSettings { hide_details: true, link_line1: true, button1: "listen".into(), app_name: "Music".into(), ..PresenceSettings::default() };
        let text = build_card(&s, &track(), true, Some("https://a.mzstatic.com/x.jpg"), NOW).to_json().to_string();
        for secret in ["Carrier", "Paper", "Signal", "mzstatic", "search"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert!(text.contains("Listening to music"));
        assert!(!s.wants_cover(), "and no cover lookup is made");
    }

    #[test]
    fn text_is_kept_within_what_discord_accepts() {
        let long = "é".repeat(300);
        let t = TrackInfo { title: long, artist: "A".into(), ..TrackInfo::default() };
        let c = build_card(&PresenceSettings::default(), &t, true, None, NOW);
        assert_eq!(c.details.chars().count(), 127);
        assert!(c.details.ends_with('…'));
        assert!(c.state.as_ref().unwrap().chars().count() >= 2, "one character is padded: Discord rejects it");
        assert_eq!(build_card(&PresenceSettings::default(), &TrackInfo::default(), true, None, NOW).details, "Unknown song");
        let nan = TrackInfo { duration_secs: Some(f64::NAN), ..track() };
        assert!(build_card(&PresenceSettings::default(), &nan, true, None, NOW).end.is_none());
        let huge = TrackInfo { artist: "x".repeat(600), ..track() };
        assert!(build_card(&PresenceSettings::default(), &huge, true, None, NOW).buttons.iter().all(|b| b.url.len() <= MAX_BUTTON_URL));
    }

    #[test]
    fn frames_round_trip_and_oversized_ones_are_refused() {
        let mut buf = Vec::new();
        write_frame(&mut buf, 1, r#"{"a":1}"#).unwrap();
        assert_eq!(&buf[..8], &[1, 0, 0, 0, 7, 0, 0, 0]);
        let (op, v) = read_frame(&mut buf.as_slice()).unwrap();
        assert_eq!((op, v["a"].as_i64()), (1, Some(1)));
        let huge = [1u8, 0, 0, 0, 0xff, 0xff, 0xff, 0x7f];
        assert!(read_frame(&mut &huge[..]).is_err());
        assert!(read_frame(&mut &buf[..5]).is_err(), "a cut-off message is an error, not a hang");
    }

    #[cfg(unix)]
    mod socket {
        use super::*;
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};

        /// A stand-in for Discord: answers the handshake, then acknowledges every SET_ACTIVITY and
        /// records what it was sent. `reject` makes it answer with an error instead.
        fn fake_discord(mut s: UnixStream, log: Arc<Mutex<Vec<Value>>>, reject: bool) {
            std::thread::spawn(move || {
                let (op, hello) = match read_frame(&mut s) { Ok(x) => x, Err(_) => return };
                assert_eq!(op, OP_HANDSHAKE);
                log.lock().unwrap().push(hello);
                write_frame(&mut s, OP_FRAME, r#"{"cmd":"DISPATCH","evt":"READY","data":{}}"#).unwrap();
                while let Ok((_, v)) = read_frame(&mut s) {
                    log.lock().unwrap().push(v.clone());
                    // a ping in between must be answered by the client
                    write_frame(&mut s, OP_PING, r#"{"n":1}"#).unwrap();
                    let reply = if reject { json!({"cmd":"SET_ACTIVITY","evt":"ERROR","data":{"message":"bad activity"},"nonce":v["nonce"]}) }
                        else { json!({"cmd":"SET_ACTIVITY","evt":null,"data":{},"nonce":v["nonce"]}) };
                    write_frame(&mut s, OP_FRAME, &reply.to_string()).unwrap();
                }
            });
        }

        fn runner_with_fake(reject: bool) -> (Runner, Arc<Mutex<Vec<Value>>>) {
            let log = Arc::new(Mutex::new(Vec::new()));
            let l2 = log.clone();
            let r = Runner::with_connector(Box::new(move || {
                let (a, b) = UnixStream::pair()?;
                fake_discord(b, l2.clone(), reject);
                Ok(Box::new(a) as Stream)
            }));
            (r, log)
        }

        fn on() -> PresenceSettings { PresenceSettings { enabled: true, client_id: "123456789012345678".into(), ..PresenceSettings::default() } }
        fn sets(log: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> { log.lock().unwrap().iter().filter(|v| v["cmd"] == "SET_ACTIVITY").cloned().collect() }

        #[test]
        fn the_handshake_sends_the_application_id_and_the_card_arrives() {
            let (mut r, log) = runner_with_fake(false);
            r.tick(&on(), song(1, NOW), Instant::now());
            assert_eq!(r.status().state, "connected");
            let log2 = log.lock().unwrap().clone();
            assert_eq!(log2[0]["client_id"], "123456789012345678");
            assert_eq!(log2[0]["v"], 1);
            let s = sets(&log);
            assert_eq!(s.len(), 1);
            assert_eq!(s[0]["args"]["activity"]["details"], "Song 1");
            assert_eq!(s[0]["args"]["activity"]["type"], 2);
        }

        #[test]
        fn an_unchanged_song_is_not_sent_again_but_a_new_song_a_seek_and_a_pause_are() {
            let (mut r, log) = runner_with_fake(false);
            let t = Instant::now();
            let sec = |n: u64| t + Duration::from_secs(n);
            r.tick(&on(), song(1, NOW), sec(0));
            r.tick(&on(), song(1, NOW + 1000), sec(2)); // rounding only
            assert_eq!(sets(&log).len(), 1);
            r.tick(&on(), song(1, NOW + 60_000), sec(4)); // seeked back a minute
            assert_eq!(sets(&log).len(), 2);
            r.tick(&on(), song(2, NOW + 60_000), sec(6)); // next song
            assert_eq!(sets(&log).len(), 3);
            r.tick(&on(), None, sec(8)); // stopped
            let s = sets(&log);
            assert_eq!(s.len(), 4);
            assert!(s[3]["args"].get("activity").is_none(), "nothing to show clears the line");
            r.tick(&on(), None, sec(10));
            assert_eq!(sets(&log).len(), 4, "nothing playing and nothing shown is not an update");
        }

        #[test]
        fn a_changed_setting_updates_the_card_and_an_old_card_is_refreshed() {
            let (mut r, log) = runner_with_fake(false);
            let t = Instant::now();
            let with_title = |c: &str| Some(Want { entry: 1, card: Card { details: c.into(), large_image: LOGO_ASSET.into(), ..Card::default() } });
            r.tick(&on(), with_title("a"), t);
            r.tick(&on(), with_title("b"), t + Duration::from_secs(2));
            assert_eq!(sets(&log).len(), 2);
            r.tick(&on(), with_title("b"), t + Duration::from_secs(30));
            assert_eq!(sets(&log).len(), 2);
            r.tick(&on(), with_title("b"), t + Duration::from_secs(48));
            assert_eq!(sets(&log).len(), 3, "sent again after the refresh time, in case Discord lost it");
        }

        #[test]
        fn skipping_through_songs_is_not_sent_faster_than_discord_allows() {
            let (mut r, log) = runner_with_fake(false);
            let t = Instant::now();
            r.tick(&on(), song(1, NOW), t);
            r.tick(&on(), song(2, NOW), t + Duration::from_millis(300));
            assert_eq!(sets(&log).len(), 1, "too soon");
            r.tick(&on(), song(2, NOW), t + Duration::from_secs(2));
            let s = sets(&log);
            assert_eq!(s.len(), 2);
            assert_eq!(s[1]["args"]["activity"]["details"], "Song 2", "the latest one wins, not each in between");
        }

        #[test]
        fn no_more_than_five_updates_go_out_in_one_window() {
            let (mut r, log) = runner_with_fake(false);
            let t = Instant::now();
            for i in 0..8u64 {
                r.tick(&on(), song(i + 1, NOW), t + Duration::from_millis(2000 * i));
            }
            assert_eq!(sets(&log).len(), 5, "the rest wait for the window to pass");
            r.tick(&on(), song(8, NOW), t + Duration::from_secs(23));
            let s = sets(&log);
            assert_eq!(s.len(), 6);
            assert_eq!(s[5]["args"]["activity"]["details"], "Song 8");
        }

        #[test]
        fn switching_it_off_takes_the_line_down_and_lets_go() {
            let (mut r, log) = runner_with_fake(false);
            let t = Instant::now();
            r.tick(&on(), song(1, NOW), t);
            r.tick(&PresenceSettings { enabled: false, ..on() }, song(1, NOW), t + Duration::from_secs(3));
            assert_eq!(r.status().state, "off");
            let s = sets(&log);
            assert_eq!(s.len(), 2);
            assert!(s[1]["args"].get("activity").is_none());
            // and while off it never connects again
            r.tick(&PresenceSettings { enabled: false, ..on() }, song(1, NOW), t + Duration::from_secs(60));
            assert_eq!(log.lock().unwrap().iter().filter(|v| v.get("client_id").is_some()).count(), 1);
        }

        #[test]
        fn the_built_in_application_id_is_used_when_none_is_typed() {
            let (mut r, log) = runner_with_fake(false);
            r.tick(&PresenceSettings { enabled: true, ..PresenceSettings::default() }, song(1, NOW), Instant::now());
            assert_eq!(r.status().state, "connected");
            assert_eq!(log.lock().unwrap()[0]["client_id"], DEFAULT_CLIENT_ID);
        }

        #[test]
        fn a_rejected_card_is_reported_and_retried_later_not_in_a_loop() {
            let (mut r, log) = runner_with_fake(true);
            let t = Instant::now();
            r.tick(&on(), song(1, NOW), t);
            assert_eq!(r.status().state, "error");
            assert!(r.status().detail.contains("bad activity"));
            r.tick(&on(), song(1, NOW), t + Duration::from_secs(1));
            assert_eq!(sets(&log).len(), 1, "waits before trying again");
            r.tick(&on(), song(1, NOW), t + Duration::from_secs(6));
            assert_eq!(sets(&log).len(), 2);
        }
    }

    #[test]
    fn when_discord_is_not_running_it_waits_and_tries_again_later() {
        let tries = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let t2 = tries.clone();
        let mut r = Runner::with_connector(Box::new(move || {
            t2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(io::Error::new(io::ErrorKind::NotFound, "Discord is not running"))
        }));
        let on = PresenceSettings { enabled: true, client_id: "123456789012345678".into(), ..PresenceSettings::default() };
        let t = Instant::now();
        r.tick(&on, song(1, NOW), t);
        assert_eq!(r.status().state, "waiting");
        r.tick(&on, song(1, NOW), t + Duration::from_secs(5));
        r.tick(&on, song(1, NOW), t + Duration::from_secs(10));
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 1, "no hammering");
        r.tick(&on, song(1, NOW), t + Duration::from_secs(16));
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
