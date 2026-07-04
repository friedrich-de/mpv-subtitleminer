use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::time::{Duration, timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::media::FfmpegRequest;
use crate::mpv_stream::MpvStream;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SubtitleTrack {
    Primary,
    Secondary,
}

impl SubtitleTrack {
    fn as_str(self) -> &'static str {
        match self {
            SubtitleTrack::Primary => "primary",
            SubtitleTrack::Secondary => "secondary",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubtitleMode {
    AssFull,
    Legacy,
}

impl SubtitleMode {
    fn as_str(self) -> &'static str {
        match self {
            SubtitleMode::AssFull => "ass-full",
            SubtitleMode::Legacy => "legacy",
        }
    }
}

fn parse_mpv_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut tokens = s.split_whitespace();
    if !tokens.next()?.eq_ignore_ascii_case("mpv") {
        return None;
    }

    let token = tokens.next()?;
    let numeric: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = numeric.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn subtitle_mode_from_mpv_version(s: &str) -> Option<SubtitleMode> {
    let version = parse_mpv_version(s)?;
    if version >= (0, 39, 0) {
        Some(SubtitleMode::AssFull)
    } else {
        Some(SubtitleMode::Legacy)
    }
}

/// Parses an ASS timestamp `H:MM:SS.cc` (centiseconds) into absolute seconds.
fn parse_ass_time(s: &str) -> Option<f64> {
    let mut parts = s.trim().split(':');
    let h: f64 = parts.next()?.trim().parse().ok()?;
    let m: f64 = parts.next()?.trim().parse().ok()?;
    let sec: f64 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Converts an ASS event Text field to display text: drops `{...}` override
/// blocks and converts hard breaks (`\N`, `\n`) to newlines and hard spaces
/// (`\h`) to spaces.
fn strip_ass_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_override = false;
    while let Some(c) = chars.next() {
        match c {
            '{' => in_override = true,
            '}' => in_override = false,
            _ if in_override => {}
            '\\' => match chars.peek() {
                Some('N') | Some('n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('h') => {
                    out.push(' ');
                    chars.next();
                }
                _ => out.push('\\'),
            },
            _ => out.push(c),
        }
    }
    out
}

/// Extracts `(start, end, style, name, text)` from a single ASS Dialogue line:
/// `[Dialogue: ]Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text`.
/// Times are absolute seconds; `text` is display-cleaned. Returns `None` for any
/// line that isn't a parseable Dialogue (headers, Comment lines, etc.).
fn parse_ass_dialogue(s: &str) -> Option<(f64, f64, &str, &str, String)> {
    let s = s.strip_prefix("Dialogue: ").unwrap_or(s);
    let mut parts = s.splitn(10, ',');
    // Layer is always an integer in a real Dialogue event; this also rejects
    // Comment lines and headers that happen to share the comma layout.
    parts.next()?.trim().parse::<i64>().ok()?;
    let start = parse_ass_time(parts.next()?)?;
    let end = parse_ass_time(parts.next()?)?;
    let style = parts.next()?.trim();
    let name = parts.next()?.trim();
    parts.next()?; // MarginL
    parts.next()?; // MarginR
    parts.next()?; // MarginV
    parts.next()?; // Effect
    let text = parts.next()?;
    Some((start, end, style, name, strip_ass_text(text)))
}

#[derive(Clone)]
pub struct Subtitle {
    pub id: u64,
    pub text: String,
    pub sub_start: f64,
    pub sub_end: f64,
    pub media_path: String,
    pub aid: i64,
    pub track: SubtitleTrack,
    /// ASS Style field; empty for non-ASS tracks (mpv reports SRT as `Default`).
    pub style: String,
    /// ASS Name/Actor field; empty when unset or for non-ASS tracks.
    pub name: String,
}

#[derive(Clone)]
pub enum SubtitleEvent {
    New(Subtitle),
    MediaChanged(String),
}

/// Identity of an emitted subtitle: track, a coarse start-time bucket (see
/// `DEDUP_BUCKET_MS`), and ASS Style/Name/text. The bucket lets mpv's exact
/// re-reports of an overlapping event collapse while keeping the same text said
/// again later as a distinct row.
type SubtitleKey = (SubtitleTrack, i64, String, String, String);

/// Width of the start-time bucket used in `SubtitleKey`, in milliseconds.
const DEDUP_BUCKET_MS: i64 = 5000;

/// Bounded set of recently emitted subtitle identities. mpv re-reports the same
/// `Dialogue` line whenever an overlapping neighbor enters or leaves the screen,
/// so without this every overlap spawns duplicate rows. FIFO eviction keeps
/// memory flat; the cap bounds how far a backward seek can re-enter watched
/// territory before lines re-emit.
struct RecentSubtitles {
    seen: HashSet<SubtitleKey>,
    order: VecDeque<SubtitleKey>,
    cap: usize,
}

impl RecentSubtitles {
    fn new(cap: usize) -> Self {
        Self {
            seen: HashSet::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    /// Records `key`; returns `true` if it was newly seen (caller should emit),
    /// `false` if it's a duplicate to drop.
    fn insert(&mut self, key: SubtitleKey) -> bool {
        if !self.seen.insert(key.clone()) {
            return false;
        }
        self.order.push_back(key);
        if self.order.len() > self.cap
            && let Some(old) = self.order.pop_front()
        {
            self.seen.remove(&old);
        }
        true
    }

    fn clear(&mut self) {
        self.seen.clear();
        self.order.clear();
    }
}

struct SharedState {
    subtitles: RwLock<HashMap<u64, Subtitle>>,
    current_media_path: RwLock<Option<String>>,
    /// Dedup set for the duplicate Dialogue lines mpv re-reports around overlaps.
    /// Held here (not just in `handle_mpv`) so a client `clear` request can reset
    /// it; reset on media change.
    recent: Mutex<RecentSubtitles>,
}

impl SharedState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            subtitles: RwLock::new(HashMap::new()),
            current_media_path: RwLock::new(None),
            recent: Mutex::new(RecentSubtitles::new(512)),
        })
    }
}

fn build_legacy_subtitle(
    id: u64,
    text: String,
    timing: [Option<f64>; 2],
    media_path: String,
    aid: i64,
    track: SubtitleTrack,
    delay: f64,
) -> Option<Subtitle> {
    let raw_start = timing[0]?;
    let raw_end = timing[1]?;
    Some(Subtitle {
        id,
        text,
        sub_start: raw_start + delay,
        sub_end: raw_end + delay,
        media_path,
        aid,
        track,
        style: String::new(),
        name: String::new(),
    })
}

struct PendingLegacySubtitle {
    id: u64,
    text: String,
    track: SubtitleTrack,
    media_path: String,
    aid: i64,
    delay: f64,
    timing: [Option<f64>; 2],
    received: [bool; 2],
}

impl PendingLegacySubtitle {
    fn new(
        id: u64,
        text: String,
        track: SubtitleTrack,
        media_path: String,
        aid: i64,
        delay: f64,
    ) -> Self {
        Self {
            id,
            text,
            track,
            media_path,
            aid,
            delay,
            timing: [None, None],
            received: [false, false],
        }
    }

    fn set_timing_response(&mut self, index: usize, value: Option<&serde_json::Value>) {
        if index >= self.timing.len() {
            return;
        }
        self.received[index] = true;
        self.timing[index] = value.and_then(|v| v.as_f64());
    }

    fn is_complete(&self) -> bool {
        self.received.iter().all(|received| *received)
    }

    fn raw_start(&self) -> Option<f64> {
        self.timing[0]
    }

    fn to_subtitle(&self) -> Option<Subtitle> {
        build_legacy_subtitle(
            self.id,
            self.text.clone(),
            self.timing,
            self.media_path.clone(),
            self.aid,
            self.track,
            self.delay,
        )
    }
}

async fn query_mpv_property(
    mpv: &mut MpvStream,
    property: &str,
    request_id: u64,
) -> std::io::Result<serde_json::Value> {
    let cmd = format!(
        "{{\"command\":[\"get_property\",\"{}\"],\"request_id\":{}}}\n",
        property, request_id
    );
    mpv.write_all(cmd.as_bytes()).await?;

    let mut line = String::new();
    loop {
        line.clear();
        if mpv.read_line(&mut line).await? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "mpv IPC closed while waiting for property response",
            ));
        }
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line)
            && json.get("request_id").and_then(|v| v.as_u64()) == Some(request_id)
        {
            return Ok(json);
        }
    }
}

async fn query_mpv_property_with_timeout(
    mpv: &mut MpvStream,
    property: &str,
    request_id: u64,
) -> std::io::Result<serde_json::Value> {
    timeout(
        Duration::from_secs(1),
        query_mpv_property(mpv, property, request_id),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("Timed out querying mpv property '{}'", property),
        )
    })?
}

async fn get_mpv_pid(mpv: &mut MpvStream) -> std::io::Result<u32> {
    let json = match query_mpv_property_with_timeout(mpv, "pid", 1).await {
        Ok(json) => json,
        Err(_) => query_mpv_property_with_timeout(mpv, "process-id", 2).await?,
    };
    let status = json.get("error").and_then(|e| e.as_str()).unwrap_or("");
    if status != "success" {
        return Err(std::io::Error::other(format!(
            "mpv returned error querying PID: {}",
            status
        )));
    }

    let pid = json
        .get("data")
        .and_then(|d| {
            d.as_u64()
                .or_else(|| d.as_i64().and_then(|n| u64::try_from(n).ok()))
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mpv returned non-integer PID",
            )
        })?;

    u32::try_from(pid)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "mpv PID out of range"))
}

async fn detect_subtitle_mode(mpv: &mut MpvStream) -> SubtitleMode {
    let mut detected_version: Option<String> = None;
    let mode = match query_mpv_property_with_timeout(mpv, "mpv-version", 3).await {
        Ok(json) => {
            let status = json.get("error").and_then(|e| e.as_str()).unwrap_or("");
            if status != "success" {
                warn!(
                    "mpv-version query returned '{}'; falling back to legacy subtitle mode",
                    status
                );
                SubtitleMode::Legacy
            } else if let Some(version) = json.get("data").and_then(|d| d.as_str()) {
                detected_version = Some(version.to_string());
                match subtitle_mode_from_mpv_version(version) {
                    Some(SubtitleMode::AssFull) => SubtitleMode::AssFull,
                    Some(SubtitleMode::Legacy) => {
                        warn!(
                            "mpv version '{}' predates sub-text/ass-full; falling back to legacy subtitle mode",
                            version
                        );
                        SubtitleMode::Legacy
                    }
                    None => {
                        warn!(
                            "Could not parse mpv version '{}'; falling back to legacy subtitle mode",
                            version
                        );
                        SubtitleMode::Legacy
                    }
                }
            } else {
                warn!("mpv-version query returned no string; falling back to legacy subtitle mode");
                SubtitleMode::Legacy
            }
        }
        Err(e) => {
            warn!(
                "Failed to query mpv-version: {}; falling back to legacy subtitle mode",
                e
            );
            SubtitleMode::Legacy
        }
    };

    info!(
        "Selected {} subtitle mode (mpv version: {})",
        mode.as_str(),
        detected_version.as_deref().unwrap_or("unknown")
    );
    mode
}

pub async fn run_server(
    socket_path: &str,
    port: u16,
    expected_mpv_pid: Option<u32>,
) -> std::io::Result<()> {
    let mut mpv = MpvStream::connect(socket_path).await?;
    if let Some(expected) = expected_mpv_pid {
        let actual = get_mpv_pid(&mut mpv).await?;
        if actual != expected {
            return Err(std::io::Error::other(format!(
                "MPV_IPC_PID_MISMATCH expected={} actual={} socket={}",
                expected, actual, socket_path
            )));
        }
    }
    let subtitle_mode = detect_subtitle_mode(&mut mpv).await;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;

    println!(
        "WebSocket server listening on {}",
        listener
            .local_addr()
            .map_or_else(|_| format!("port {}", port), |a| a.to_string())
    );

    let state = SharedState::new();
    let (subtitle_tx, _) = broadcast::channel::<SubtitleEvent>(64);

    let mpv_state = state.clone();
    let mpv_tx = subtitle_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_mpv(mpv, subtitle_mode, mpv_state, mpv_tx).await {
            error!("MPV handler error: {}", e);
        }
        info!("MPV connection closed, shutting down.");
        std::process::exit(0);
    });

    let mut client_id = 0u64;
    loop {
        let (stream, addr) = listener.accept().await?;
        client_id += 1;
        let id = client_id;

        let client_state = state.clone();
        let client_rx = subtitle_tx.subscribe();

        tokio::spawn(async move {
            info!("[client:{}] Connected from {}", id, addr);
            if let Err(e) = handle_client(stream, id, client_state, client_rx).await {
                debug!("[client:{}] Disconnected: {}", id, e);
            } else {
                debug!("[client:{}] Disconnected", id);
            }
        });
    }
}

async fn handle_mpv(
    mut mpv: MpvStream,
    subtitle_mode: SubtitleMode,
    state: Arc<SharedState>,
    tx: broadcast::Sender<SubtitleEvent>,
) -> std::io::Result<()> {
    let observe_commands: &[u8] = match subtitle_mode {
        SubtitleMode::AssFull => {
            b"{\"command\":[\"observe_property\",1,\"sub-text/ass-full\"]}\n\
          {\"command\":[\"observe_property\",2,\"secondary-sub-text/ass-full\"]}\n\
          {\"command\":[\"observe_property\",3,\"path\"]}\n\
          {\"command\":[\"observe_property\",4,\"aid\"]}\n\
          {\"command\":[\"observe_property\",5,\"sub-delay\"]}\n\
          {\"command\":[\"observe_property\",6,\"secondary-sub-delay\"]}\n"
        }
        SubtitleMode::Legacy => {
            b"{\"command\":[\"observe_property\",1,\"sub-text\"]}\n\
          {\"command\":[\"observe_property\",2,\"secondary-sub-text\"]}\n\
          {\"command\":[\"observe_property\",3,\"path\"]}\n\
          {\"command\":[\"observe_property\",4,\"aid\"]}\n\
          {\"command\":[\"observe_property\",5,\"sub-delay\"]}\n\
          {\"command\":[\"observe_property\",6,\"secondary-sub-delay\"]}\n"
        }
    };
    mpv.write_all(observe_commands).await?;
    info!(
        "Connected to mpv, observing subtitle changes with {} mode",
        subtitle_mode.as_str()
    );

    let mut current_path: Option<String> = None;
    // Latest selected audio track id, kept current via the `aid` observe (id 4)
    // instead of being queried per subtitle. Defaults to track 1 until mpv sends
    // the initial property-change for the observe.
    let mut current_aid: i64 = 1;
    // Latest per-track subtitle delay, kept current via the sub-delay observes
    // (id 5 primary, id 6 secondary) and applied to timing at emit time.
    let mut current_sub_delay: f64 = 0.0;
    let mut current_secondary_sub_delay: f64 = 0.0;
    let mut next_subtitle_id = 1u64;
    let mut pending_legacy: HashMap<u64, PendingLegacySubtitle> = HashMap::new();
    let mut next_legacy_request_id = 10u64;
    let mut line = String::new();

    loop {
        line.clear();
        if mpv.read_line(&mut line).await? == 0 {
            return Ok(()); // EOF
        }

        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        if subtitle_mode == SubtitleMode::Legacy
            && let Some(request_id) = json.get("request_id").and_then(|r| r.as_u64())
        {
            let base_id = request_id / 10 * 10;
            let prop_idx = (request_id % 10) as usize;
            if prop_idx < 2 {
                let completed = if let Some(pending) = pending_legacy.get_mut(&base_id) {
                    pending.set_timing_response(prop_idx, json.get("data"));
                    pending.is_complete()
                } else {
                    false
                };

                if completed {
                    let pending = pending_legacy.remove(&base_id).unwrap();
                    let Some(raw_start) = pending.raw_start() else {
                        debug!(
                            "[{}:{}] Dropping legacy subtitle due to missing start timing",
                            pending.track.as_str(),
                            pending.id
                        );
                        continue;
                    };
                    let Some(sub) = pending.to_subtitle() else {
                        debug!(
                            "[{}:{}] Dropping legacy subtitle due to missing timing",
                            pending.track.as_str(),
                            pending.id
                        );
                        continue;
                    };

                    let bucket = (raw_start * 1000.0).round() as i64 / DEDUP_BUCKET_MS;
                    let key = (
                        sub.track,
                        bucket,
                        sub.style.clone(),
                        sub.name.clone(),
                        sub.text.clone(),
                    );
                    if !state.recent.lock().await.insert(key) {
                        continue;
                    }

                    debug!("[{}:{}] Broadcasting", sub.track.as_str(), sub.id);
                    info!("[{}:{}] {}", sub.track.as_str(), sub.id, sub.text);
                    state.subtitles.write().await.insert(sub.id, sub.clone());
                    let _ = tx.send(SubtitleEvent::New(sub));
                }
                continue;
            }
        }

        if json.get("event") != Some(&serde_json::json!("property-change")) {
            continue;
        }

        let observer_id = json.get("id").and_then(|v| v.as_u64());

        // Media file path changed (observer id 3)
        if observer_id == Some(3) {
            if let Some(path) = json.get("data").and_then(|d| d.as_str()) {
                let path = path.to_string();
                if current_path.as_deref() != Some(&path) {
                    current_path = Some(path.clone());
                    state.recent.lock().await.clear();
                    *state.current_media_path.write().await = Some(path.clone());
                    let _ = tx.send(SubtitleEvent::MediaChanged(path));
                }
            }
            continue;
        }

        // Audio track changed (observer id 4)
        if observer_id == Some(4) {
            if let Some(aid) = json.get("data").and_then(|d| d.as_i64()) {
                current_aid = aid;
            }
            continue;
        }

        // Subtitle delay changed: primary (id 5) or secondary (id 6).
        if observer_id == Some(5) || observer_id == Some(6) {
            if let Some(delay) = json.get("data").and_then(|d| d.as_f64()) {
                if observer_id == Some(5) {
                    current_sub_delay = delay;
                } else {
                    current_secondary_sub_delay = delay;
                }
            }
            continue;
        }

        // Subtitle changed: primary (observer id 1) or secondary (observer id 2).
        let track = match observer_id {
            Some(1) => SubtitleTrack::Primary,
            Some(2) => SubtitleTrack::Secondary,
            _ => continue,
        };

        let Some(payload) = json.get("data").and_then(|d| d.as_str()) else {
            continue;
        };

        let delay = match track {
            SubtitleTrack::Primary => current_sub_delay,
            SubtitleTrack::Secondary => current_secondary_sub_delay,
        };
        let media_path = current_path.clone().unwrap_or_default();

        match subtitle_mode {
            SubtitleMode::AssFull => {
                // Each Dialogue line carries its own absolute Start/End, so overlapping
                // events become independent rows with correct timing.
                for dialogue in payload.lines() {
                    let Some((raw_start, raw_end, style, name, text)) =
                        parse_ass_dialogue(dialogue)
                    else {
                        continue;
                    };
                    if text.is_empty() {
                        continue;
                    }

                    // Bucket the raw (pre-delay) start so re-reports collapse but the
                    // same text recurring later stays a distinct row.
                    let bucket = (raw_start * 1000.0).round() as i64 / DEDUP_BUCKET_MS;
                    let key = (
                        track,
                        bucket,
                        style.to_string(),
                        name.to_string(),
                        text.clone(),
                    );
                    if !state.recent.lock().await.insert(key) {
                        continue;
                    }

                    let subtitle_id = next_subtitle_id;
                    next_subtitle_id += 1;

                    let sub = Subtitle {
                        id: subtitle_id,
                        text,
                        sub_start: raw_start + delay,
                        sub_end: raw_end + delay,
                        media_path: media_path.clone(),
                        aid: current_aid,
                        track,
                        style: style.to_string(),
                        name: name.to_string(),
                    };
                    debug!("[{}:{}] Broadcasting", track.as_str(), subtitle_id);
                    info!("[{}:{}] {}", track.as_str(), subtitle_id, sub.text);
                    state
                        .subtitles
                        .write()
                        .await
                        .insert(subtitle_id, sub.clone());
                    let _ = tx.send(SubtitleEvent::New(sub));
                }
            }
            SubtitleMode::Legacy => {
                if payload.is_empty() {
                    continue;
                }

                let subtitle_id = next_subtitle_id;
                next_subtitle_id += 1;

                let base_id = next_legacy_request_id;
                next_legacy_request_id += 10;

                let (start_property, end_property) = match track {
                    SubtitleTrack::Primary => ("sub-start", "sub-end"),
                    SubtitleTrack::Secondary => ("secondary-sub-start", "secondary-sub-end"),
                };
                let cmd = format!(
                    concat!(
                        "{{\"command\":[\"get_property\",\"{}\"],\"request_id\":{}}}\n",
                        "{{\"command\":[\"get_property\",\"{}\"],\"request_id\":{}}}\n"
                    ),
                    start_property,
                    base_id,
                    end_property,
                    base_id + 1
                );

                mpv.write_all(cmd.as_bytes()).await?;
                pending_legacy.insert(
                    base_id,
                    PendingLegacySubtitle::new(
                        subtitle_id,
                        payload.to_string(),
                        track,
                        media_path,
                        current_aid,
                        delay,
                    ),
                );
            }
        }
        continue;
    }
}

async fn handle_client(
    stream: TcpStream,
    id: u64,
    state: Arc<SharedState>,
    mut subtitle_rx: broadcast::Receiver<SubtitleEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws = accept_async(stream).await?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    if let Some(path) = state.current_media_path.read().await.clone() {
        let msg = serde_json::json!({ "type": "media_changed", "path": path });
        ws_tx.send(Message::Text(msg.to_string().into())).await?;
    }

    loop {
        tokio::select! {
            Ok(event) = subtitle_rx.recv() => {
                let msg = match event {
                    SubtitleEvent::New(sub) => serde_json::json!({
                        "type": "subtitle",
                        "id": sub.id,
                        "subtitle": sub.text,
                        "sub_start": sub.sub_start,
                        "sub_end": sub.sub_end,
                        "track": sub.track.as_str(),
                        "style": sub.style,
                        "name": sub.name,
                    }),
                    SubtitleEvent::MediaChanged(path) => serde_json::json!({
                        "type": "media_changed",
                        "path": path,
                    }),
                };
                ws_tx.send(Message::Text(msg.to_string().into())).await?;
            }

            Some(msg) = ws_rx.next() => {
                let msg = msg?;
                if let Message::Text(text) = msg {
                    if let Some(response) = handle_request(&text, id, &state).await {
                        ws_tx.send(Message::Text(response.into())).await?;
                    }
                } else if msg.is_close() {
                    return Ok(());
                }
            }

            else => return Ok(()),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
enum ProtocolRequest {
    Thumbnail {
        id: u64,
        end_id: Option<u64>,
        image_config: Option<crate::media::ImageConfig>,
    },
    Audio {
        id: u64,
        offset_start: Option<f64>,
        offset_end: Option<f64>,
        audio_config: Option<crate::media::AudioConfig>,
    },
    AudioRange {
        start_id: u64,
        end_id: u64,
        offset_start: Option<f64>,
        offset_end: Option<f64>,
        audio_config: Option<crate::media::AudioConfig>,
    },
    /// The UI "clear" button: reset the dedup set so already-seen subtitles can
    /// stream in again (e.g. after clearing the screen and seeking back).
    Clear,
}

async fn handle_request(text: &str, client_id: u64, state: &Arc<SharedState>) -> Option<String> {
    let request: ProtocolRequest = serde_json::from_str(text).ok()?;

    match request {
        ProtocolRequest::Clear => {
            state.recent.lock().await.clear();
            info!("[client:{}] Cleared subtitle dedup set", client_id);
            None
        }
        ProtocolRequest::AudioRange {
            start_id,
            end_id,
            offset_start,
            offset_end,
            audio_config,
        } => {
            let store = state.subtitles.read().await;
            let start = store.get(&start_id)?;
            let end = store.get(&end_id)?;
            let ffmpeg_req = FfmpegRequest::audio_range(
                start.sub_start,
                end.sub_end,
                &start.media_path,
                start.aid,
                offset_start,
                offset_end,
                audio_config,
            );
            drop(store);

            info!(
                "[client:{}] Requesting audio_range from subtitle {} to {}",
                client_id, start_id, end_id
            );

            let data = tokio::task::spawn_blocking(move || ffmpeg_req.execute())
                .await
                .ok()?;

            Some(
                serde_json::json!({
                    "type": "audio_range",
                    "start_id": start_id,
                    "end_id": end_id,
                    "data": data,
                })
                .to_string(),
            )
        }
        _ => {
            let (subtitle_id, media_type, ffmpeg_req) = match request {
                ProtocolRequest::Thumbnail {
                    id,
                    end_id,
                    image_config,
                } => {
                    let store = state.subtitles.read().await;
                    let mut sub = store.get(&id)?.clone();
                    if let Some(eid) = end_id
                        && let Some(end_sub) = store.get(&eid)
                    {
                        sub.sub_end = end_sub.sub_end;
                    }
                    drop(store);
                    (
                        id,
                        "thumbnail",
                        FfmpegRequest::thumbnail(&sub, image_config),
                    )
                }
                ProtocolRequest::Audio {
                    id,
                    offset_start,
                    offset_end,
                    audio_config,
                } => {
                    let store = state.subtitles.read().await;
                    let sub = store.get(&id)?.clone();
                    drop(store);
                    (
                        id,
                        "audio",
                        FfmpegRequest::audio(&sub, offset_start, offset_end, audio_config),
                    )
                }
                _ => unreachable!(),
            };

            info!(
                "[client:{}] Requesting {} for subtitle {}",
                client_id, media_type, subtitle_id
            );

            let req_type = media_type.to_string();
            let data = tokio::task::spawn_blocking(move || ffmpeg_req.execute())
                .await
                .ok()?;

            if data.is_some() {
                debug!("[media] {} ready for subtitle {}", req_type, subtitle_id);
            } else {
                warn!(
                    "[media] Failed to generate {} for subtitle {}",
                    req_type, subtitle_id
                );
            }

            Some(
                serde_json::json!({
                    "type": req_type,
                    "id": subtitle_id,
                    "data": data,
                })
                .to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpv_version_parser_handles_common_versions() {
        assert_eq!(parse_mpv_version("mpv 0.36.0"), Some((0, 36, 0)));
        assert_eq!(parse_mpv_version("mpv 0.39.0"), Some((0, 39, 0)));
        assert_eq!(
            parse_mpv_version("mpv 0.40.1-123-gabcdef"),
            Some((0, 40, 1))
        );
        assert_eq!(parse_mpv_version("mpv-x86_64-v3 0.40.1"), None);
        assert_eq!(parse_mpv_version("custom 2024.09.01"), None);
        assert_eq!(parse_mpv_version("not a version"), None);
    }

    #[test]
    fn subtitle_mode_selects_legacy_for_old_or_unknown_versions() {
        assert_eq!(
            subtitle_mode_from_mpv_version("mpv 0.36.0"),
            Some(SubtitleMode::Legacy)
        );
        assert_eq!(subtitle_mode_from_mpv_version("custom build"), None);
    }

    #[test]
    fn subtitle_mode_selects_ass_full_for_mpv_0_39_or_newer() {
        assert_eq!(
            subtitle_mode_from_mpv_version("mpv 0.39.0"),
            Some(SubtitleMode::AssFull)
        );
        assert_eq!(
            subtitle_mode_from_mpv_version("mpv 0.40.1"),
            Some(SubtitleMode::AssFull)
        );
    }

    #[test]
    fn legacy_subtitle_builder_applies_delay_and_defaults_metadata() {
        let sub = build_legacy_subtitle(
            7,
            "hello".to_string(),
            [Some(1.0), Some(3.5)],
            "movie.mkv".to_string(),
            2,
            SubtitleTrack::Secondary,
            0.25,
        )
        .unwrap();

        assert_eq!(sub.id, 7);
        assert_eq!(sub.text, "hello");
        assert_eq!(sub.sub_start, 1.25);
        assert_eq!(sub.sub_end, 3.75);
        assert_eq!(sub.media_path, "movie.mkv");
        assert_eq!(sub.aid, 2);
        assert_eq!(sub.track, SubtitleTrack::Secondary);
        assert_eq!(sub.style, "");
        assert_eq!(sub.name, "");
    }

    #[test]
    fn legacy_subtitle_builder_drops_missing_timing() {
        assert!(
            build_legacy_subtitle(
                1,
                "hello".to_string(),
                [None, Some(3.5)],
                "movie.mkv".to_string(),
                1,
                SubtitleTrack::Primary,
                0.0,
            )
            .is_none()
        );
        assert!(
            build_legacy_subtitle(
                1,
                "hello".to_string(),
                [Some(1.0), None],
                "movie.mkv".to_string(),
                1,
                SubtitleTrack::Primary,
                0.0,
            )
            .is_none()
        );
    }

    #[test]
    fn ass_time_parses_h_mm_ss_cc() {
        assert_eq!(parse_ass_time("0:00:01.50"), Some(1.5));
        assert_eq!(parse_ass_time("0:00:00.00"), Some(0.0));
        assert_eq!(parse_ass_time("1:02:03.00"), Some(3723.0));
        assert_eq!(parse_ass_time(" 0:00:01.50 "), Some(1.5));
    }

    #[test]
    fn ass_time_rejects_malformed() {
        assert_eq!(parse_ass_time("00:01.50"), None); // missing hours field
        assert_eq!(parse_ass_time("0:00:01:50"), None); // too many fields
        assert_eq!(parse_ass_time("a:b:c"), None);
    }

    #[test]
    fn strip_ass_text_removes_overrides_and_breaks() {
        assert_eq!(strip_ass_text("{\\i1}Hello{\\i0}"), "Hello");
        assert_eq!(strip_ass_text("Line1\\NLine2"), "Line1\nLine2");
        assert_eq!(strip_ass_text("a\\hb"), "a b");
        assert_eq!(strip_ass_text("plain"), "plain");
        // backslash not introducing a known escape is kept verbatim
        assert_eq!(strip_ass_text("a\\xb"), "a\\xb");
    }

    #[test]
    fn parse_dialogue_extracts_fields() {
        let line = "Dialogue: 0,0:00:01.00,0:00:03.50,Default,Alice,0,0,0,,{\\i1}Hi{\\i0} there";
        let (start, end, style, name, text) = parse_ass_dialogue(line).unwrap();
        assert_eq!(start, 1.0);
        assert_eq!(end, 3.5);
        assert_eq!(style, "Default");
        assert_eq!(name, "Alice");
        assert_eq!(text, "Hi there");
    }

    #[test]
    fn parse_dialogue_handles_srt_converted_and_commas_in_text() {
        // SRT converted by mpv: single Default style, empty Name, plain text.
        let line = "Dialogue: 0,0:00:05.00,0:00:07.00,Default,,0,0,0,,Wait, stop!";
        let (start, end, style, name, text) = parse_ass_dialogue(line).unwrap();
        assert_eq!((start, end), (5.0, 7.0));
        assert_eq!(style, "Default");
        assert_eq!(name, "");
        assert_eq!(text, "Wait, stop!"); // comma in the Text field is preserved
    }

    #[test]
    fn parse_dialogue_rejects_non_dialogue() {
        assert!(parse_ass_dialogue("Comment: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,x").is_none());
        assert!(parse_ass_dialogue("[Script Info]").is_none());
    }

    fn key(text: &str) -> SubtitleKey {
        (
            SubtitleTrack::Primary,
            0,
            "Default".to_string(),
            String::new(),
            text.to_string(),
        )
    }

    #[test]
    fn recent_subtitles_dedups_repeats() {
        let mut recent = RecentSubtitles::new(4);
        assert!(recent.insert(key("a"))); // first sighting -> emit
        assert!(!recent.insert(key("a"))); // re-report -> drop
        assert!(recent.insert(key("b")));
        recent.clear();
        assert!(recent.insert(key("a"))); // cleared -> emit again
    }

    #[test]
    fn recent_subtitles_evicts_oldest_past_cap() {
        let mut recent = RecentSubtitles::new(2);
        recent.insert(key("a"));
        recent.insert(key("b"));
        recent.insert(key("c")); // evicts "a"
        assert!(recent.insert(key("a"))); // "a" no longer remembered -> emit
        assert!(!recent.insert(key("c"))); // "c" still within window -> drop
    }
}
