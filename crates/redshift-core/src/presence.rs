//! Discord rich presence: "Listening to <song> by <artist>" on the person's Discord profile.
//!
//! Redshift talks to the Discord app running on the same computer through its local pipe; Discord
//! does the rest, so this never touches the network itself and is separate from the online switch.
//! It is off until switched on, and the person chooses whether to share the song or only the fact
//! that they are listening to music. Everything here is plain `std`: the framing is eight bytes
//! (opcode, length) followed by JSON.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The Discord application this build shows itself as. Empty until one is registered at
/// discord.com/developers (the name there is what Discord shows after "Listening to"); the person can
/// also paste their own in Settings.
pub const DEFAULT_CLIENT_ID: &str = "";

/// The name of the image asset uploaded to the Discord application, shown as the large picture.
const LARGE_IMAGE: &str = "redshift";
/// Discord ignores activity updates that come faster than this.
const MIN_GAP: Duration = Duration::from_millis(1500);
const RETRY_NOT_RUNNING: Duration = Duration::from_secs(15);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(5);
/// A song's start moves by a second or two from rounding; more than this is a seek.
const SEEK_SLACK_SECS: i64 = 3;

/// What is shown on the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceShare {
    /// The song, artist and album, with the time left.
    #[default]
    Song,
    /// Only "Listening to music": nothing about what.
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresenceSettings {
    pub enabled: bool,
    pub share: PresenceShare,
    /// A Discord application id; empty uses [`DEFAULT_CLIENT_ID`].
    pub client_id: String,
}

impl Default for PresenceSettings {
    fn default() -> Self {
        Self { enabled: false, share: PresenceShare::Song, client_id: String::new() }
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

/// One activity as Discord shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub details: String,
    pub state: Option<String>,
    pub album: Option<String>,
    /// Unix seconds.
    pub start: i64,
    pub end: Option<i64>,
}

/// Discord wants 2 to 128 characters in every text field.
fn clip(s: &str) -> String {
    let s = s.trim();
    let mut out: String = s.chars().take(120).collect();
    if s.chars().count() > 120 {
        out.pop();
        out.push('…');
    }
    while out.chars().count() < 2 {
        out.push('\u{a0}');
    }
    out
}

/// The activity for a song that is playing `position` seconds in.
pub fn activity_for(share: PresenceShare, title: &str, artist: &str, album: &str, duration: Option<f64>, position: f64, now_unix: i64) -> Activity {
    let start = now_unix - position.max(0.0) as i64;
    match share {
        PresenceShare::Generic => Activity { details: "Listening to music".into(), state: None, album: None, start, end: None },
        PresenceShare::Song => Activity {
            details: clip(if title.trim().is_empty() { "Unknown song" } else { title }),
            state: (!artist.trim().is_empty()).then(|| clip(artist)),
            album: (!album.trim().is_empty()).then(|| clip(album)),
            start,
            end: duration.filter(|d| d.is_finite() && *d > 0.0).map(|d| start + d.round() as i64),
        },
    }
}

impl Activity {
    fn to_json(&self) -> Value {
        // Type 2 is "Listening to".
        let mut a = json!({ "type": 2, "details": self.details, "timestamps": { "start": self.start }, "assets": { "large_image": LARGE_IMAGE } });
        if let Some(s) = &self.state {
            a["state"] = json!(s);
        }
        if let Some(e) = self.end {
            a["timestamps"]["end"] = json!(e);
        }
        if let Some(al) = &self.album {
            a["assets"]["large_text"] = json!(al);
        }
        a
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

    /// Shows `activity`, or clears the profile line when `None`.
    pub fn set_activity(&mut self, activity: Option<&Activity>) -> io::Result<()> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        let mut args = json!({ "pid": std::process::id() });
        if let Some(a) = activity {
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
    pub activity: Activity,
}

type Connector = Box<dyn Fn() -> io::Result<Stream> + Send>;

pub struct Runner {
    connector: Connector,
    conn: Option<Client<Stream>>,
    conn_id: String,
    /// What Discord was last told.
    last: Option<(u64, Activity)>,
    last_sent: Option<Instant>,
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
        Self { connector, conn: None, conn_id: String::new(), last: None, last_sent: None, retry_at: None, status: PresenceStatus::new("off", "") }
    }

    pub fn status(&self) -> PresenceStatus {
        self.status.clone()
    }

    fn drop_conn(&mut self) {
        self.conn = None;
        self.last = None;
        self.last_sent = None;
    }

    /// Takes the line off the profile (if it was there) and lets go of Discord.
    fn shut_down(&mut self) {
        if let (Some(c), Some(_)) = (self.conn.as_mut(), self.last.as_ref()) {
            let _ = c.set_activity(None);
        }
        self.drop_conn();
        self.retry_at = None;
    }

    /// Call about once a second with what should be shown (`None` for nothing playing).
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
            (Some((id, a)), Some(w)) => {
                *id == w.entry
                    && a.details == w.activity.details
                    && a.state == w.activity.state
                    && a.end.is_some() == w.activity.end.is_some()
                    && (a.start - w.activity.start).abs() <= SEEK_SLACK_SECS
            }
            _ => false,
        };
        if same || self.last_sent.is_some_and(|t| now.duration_since(t) < MIN_GAP) {
            return;
        }
        let Some(conn) = self.conn.as_mut() else { return };
        match conn.set_activity(want.as_ref().map(|w| &w.activity)) {
            Ok(()) => {
                self.last = want.map(|w| (w.entry, w.activity));
                self.last_sent = Some(now);
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

    fn song(entry: u64, start: i64) -> Option<Want> {
        Some(Want { entry, activity: Activity { details: format!("Song {entry}"), state: Some("Artist".into()), album: None, start, end: Some(start + 200) } })
    }

    #[test]
    fn client_ids_are_checked() {
        assert!(valid_client_id("123456789012345678"));
        assert!(!valid_client_id(""));
        assert!(!valid_client_id("12345"));
        assert!(!valid_client_id("12345678901234567x"));
        let mut s = PresenceSettings::default();
        assert_eq!(s.effective_client_id(), None, "no id built in yet, and none typed");
        s.client_id = " 123456789012345678 ".into();
        assert_eq!(s.effective_client_id(), Some("123456789012345678"));
        s.client_id = "not an id".into();
        assert_eq!(s.effective_client_id(), None);
    }

    #[test]
    fn it_is_off_and_shares_the_song_by_default_and_old_settings_load() {
        let d = PresenceSettings::default();
        assert!(!d.enabled);
        assert_eq!(d.share, PresenceShare::Song);
        let loaded: PresenceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded, d);
        let generic: PresenceSettings = serde_json::from_str(r#"{"enabled":true,"share":"generic"}"#).unwrap();
        assert!(generic.enabled && generic.share == PresenceShare::Generic);
    }

    #[test]
    fn the_song_activity_has_title_artist_album_and_the_time_left() {
        let a = activity_for(PresenceShare::Song, "Carrier Wave", "Paper Satellites", "Signal & Noise", Some(200.4), 30.7, 1_000_000);
        assert_eq!(a.details, "Carrier Wave");
        assert_eq!(a.state.as_deref(), Some("Paper Satellites"));
        assert_eq!(a.start, 1_000_000 - 30);
        assert_eq!(a.end, Some(a.start + 200));
        let j = a.to_json();
        assert_eq!(j["type"], 2);
        assert_eq!(j["assets"]["large_text"], "Signal & Noise");
        assert_eq!(j["timestamps"]["end"], a.end.unwrap());
    }

    #[test]
    fn the_generic_activity_says_nothing_about_the_song() {
        let a = activity_for(PresenceShare::Generic, "Secret Title", "Secret Artist", "Secret Album", Some(200.0), 10.0, 1_000_000);
        let text = a.to_json().to_string();
        assert!(!text.contains("Secret"), "{text}");
        assert!(a.end.is_none(), "not even how long the song is");
        assert!(text.contains("Listening to music"));
    }

    #[test]
    fn text_is_kept_within_what_discord_accepts() {
        let long = "é".repeat(300);
        let a = activity_for(PresenceShare::Song, &long, "A", "", None, 0.0, 10);
        assert_eq!(a.details.chars().count(), 120);
        assert!(a.details.ends_with('…'));
        assert!(a.state.as_ref().unwrap().chars().count() >= 2, "one character is padded: Discord rejects it");
        assert_eq!(activity_for(PresenceShare::Song, "  ", "", "", None, 0.0, 10).details, "Unknown song");
        assert!(activity_for(PresenceShare::Song, "x", "", "", Some(f64::NAN), 0.0, 10).end.is_none());
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

        fn on() -> PresenceSettings { PresenceSettings { enabled: true, share: PresenceShare::Song, client_id: "123456789012345678".into() } }
        fn sets(log: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> { log.lock().unwrap().iter().filter(|v| v["cmd"] == "SET_ACTIVITY").cloned().collect() }

        #[test]
        fn the_handshake_sends_the_application_id_and_the_activity_arrives() {
            let (mut r, log) = runner_with_fake(false);
            let t = Instant::now();
            r.tick(&on(), song(1, 1000), t);
            assert_eq!(r.status().state, "connected");
            let log2 = log.lock().unwrap().clone();
            assert_eq!(log2[0]["client_id"], "123456789012345678");
            assert_eq!(log2[0]["v"], 1);
            drop(log2);
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
            r.tick(&on(), song(1, 1000), sec(0));
            r.tick(&on(), song(1, 1001), sec(2)); // rounding only
            assert_eq!(sets(&log).len(), 1);
            r.tick(&on(), song(1, 1060), sec(4)); // seeked back a minute
            assert_eq!(sets(&log).len(), 2);
            r.tick(&on(), song(2, 1060), sec(6)); // next song
            assert_eq!(sets(&log).len(), 3);
            r.tick(&on(), None, sec(8)); // paused
            let s = sets(&log);
            assert_eq!(s.len(), 4);
            assert!(s[3]["args"].get("activity").is_none(), "paused clears the line");
            r.tick(&on(), None, sec(10));
            assert_eq!(sets(&log).len(), 4, "nothing playing and nothing shown is not an update");
        }

        #[test]
        fn skipping_through_songs_is_not_sent_faster_than_discord_allows() {
            let (mut r, log) = runner_with_fake(false);
            let t = Instant::now();
            r.tick(&on(), song(1, 1000), t);
            r.tick(&on(), song(2, 1000), t + Duration::from_millis(300));
            assert_eq!(sets(&log).len(), 1, "too soon");
            r.tick(&on(), song(2, 1000), t + Duration::from_secs(2));
            let s = sets(&log);
            assert_eq!(s.len(), 2);
            assert_eq!(s[1]["args"]["activity"]["details"], "Song 2", "the latest one wins, not each in between");
        }

        #[test]
        fn switching_it_off_takes_the_line_down_and_lets_go() {
            let (mut r, log) = runner_with_fake(false);
            let t = Instant::now();
            r.tick(&on(), song(1, 1000), t);
            r.tick(&PresenceSettings { enabled: false, ..on() }, song(1, 1000), t + Duration::from_secs(3));
            assert_eq!(r.status().state, "off");
            let s = sets(&log);
            assert_eq!(s.len(), 2);
            assert!(s[1]["args"].get("activity").is_none());
            // and while off it never connects again
            r.tick(&PresenceSettings { enabled: false, ..on() }, song(1, 1000), t + Duration::from_secs(60));
            assert_eq!(log.lock().unwrap().iter().filter(|v| v.get("client_id").is_some()).count(), 1);
        }

        #[test]
        fn without_an_application_id_nothing_is_attempted() {
            let (mut r, log) = runner_with_fake(false);
            r.tick(&PresenceSettings { enabled: true, ..PresenceSettings::default() }, song(1, 1000), Instant::now());
            assert_eq!(r.status().state, "no_id");
            assert!(log.lock().unwrap().is_empty());
        }

        #[test]
        fn a_rejected_activity_is_reported_and_retried_later_not_in_a_loop() {
            let (mut r, log) = runner_with_fake(true);
            let t = Instant::now();
            r.tick(&on(), song(1, 1000), t);
            assert_eq!(r.status().state, "error");
            assert!(r.status().detail.contains("bad activity"));
            r.tick(&on(), song(1, 1000), t + Duration::from_secs(1));
            assert_eq!(sets(&log).len(), 1, "waits before trying again");
            r.tick(&on(), song(1, 1000), t + Duration::from_secs(6));
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
        let on = PresenceSettings { enabled: true, share: PresenceShare::Song, client_id: "123456789012345678".into() };
        let t = Instant::now();
        r.tick(&on, song(1, 1000), t);
        assert_eq!(r.status().state, "waiting");
        r.tick(&on, song(1, 1000), t + Duration::from_secs(5));
        r.tick(&on, song(1, 1000), t + Duration::from_secs(10));
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 1, "no hammering");
        r.tick(&on, song(1, 1000), t + Duration::from_secs(16));
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
