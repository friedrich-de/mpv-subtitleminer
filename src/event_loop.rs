use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, broadcast};
use tokio::time::{Duration, timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::media::{FfmpegRequest, MediaType};
use crate::mpv_stream::MpvStream;

#[derive(Clone)]
pub struct Subtitle {
    pub id: u64,
    pub text: String,
    pub sub_start: f64,
    pub sub_end: f64,
    pub media_path: String,
    pub aid: i64,
}

struct SharedState {
    subtitles: RwLock<HashMap<u64, Subtitle>>,
}

impl SharedState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            subtitles: RwLock::new(HashMap::new()),
        })
    }
}

struct PendingSubtitle {
    id: u64,
    text: String,
    responses: [Option<serde_json::Value>; 4], // sub_start, sub_end, path, aid
}

impl PendingSubtitle {
    fn new(id: u64, text: String) -> Self {
        Self {
            id,
            text,
            responses: Default::default(),
        }
    }

    fn set_response(&mut self, index: usize, value: serde_json::Value) {
        if index < 4 {
            self.responses[index] = Some(value);
        }
    }

    fn is_complete(&self) -> bool {
        self.responses.iter().all(|r| r.is_some())
    }

    fn into_subtitle(self) -> Subtitle {
        Subtitle {
            id: self.id,
            text: self.text,
            sub_start: self.responses[0].as_ref().unwrap().as_f64().unwrap(),
            sub_end: self.responses[1].as_ref().unwrap().as_f64().unwrap(),
            media_path: self.responses[2]
                .as_ref()
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
            aid: self.responses[3].as_ref().unwrap().as_i64().unwrap(),
        }
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
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("mpv returned error querying PID: {}", status),
        ));
    }

    let pid = json
        .get("data")
        .and_then(|d| d.as_u64().or_else(|| d.as_i64().and_then(|n| u64::try_from(n).ok())))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mpv returned non-integer PID",
            )
        })?;

    u32::try_from(pid).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "mpv PID out of range")
    })
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
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "MPV_IPC_PID_MISMATCH expected={} actual={} socket={}",
                    expected, actual, socket_path
                ),
            ));
        }
    }
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;

    println!(
        "WebSocket server listening on {}",
        listener
            .local_addr()
            .map_or_else(|_| format!("port {}", port), |a| a.to_string())
    );

    let state = SharedState::new();
    let (subtitle_tx, _) = broadcast::channel::<Subtitle>(64);

    let mpv_state = state.clone();
    let mpv_tx = subtitle_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_mpv(mpv, mpv_state, mpv_tx).await {
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
    state: Arc<SharedState>,
    tx: broadcast::Sender<Subtitle>,
) -> std::io::Result<()> {
    mpv.write_all(b"{\"command\":[\"observe_property\",1,\"sub-text\"]}\n")
        .await?;
    info!("Connected to mpv, observing subtitle changes");

    let mut pending: HashMap<u64, PendingSubtitle> = HashMap::new();
    let mut next_subtitle_id = 1u64;
    let mut next_request_id = 10u64;
    let mut line = String::new();

    loop {
        line.clear();
        if mpv.read_line(&mut line).await? == 0 {
            return Ok(()); // EOF
        }

        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        // Handle property responses (request_id encodes: base_id + property_index)
        if let Some(request_id) = json.get("request_id").and_then(|r| r.as_u64()) {
            let base_id = request_id / 10 * 10; // Round down to base
            let prop_idx = (request_id % 10) as usize;

            if let Some(p) = pending.get_mut(&base_id)
                && let Some(data) = json.get("data").cloned()
            {
                p.set_response(prop_idx, data);
            }

            // Try to complete pending subtitles
            let completed: Vec<_> = pending
                .iter()
                .filter(|(_, p)| p.is_complete())
                .map(|(id, _)| *id)
                .collect();

            for base_id in completed {
                let sub = pending.remove(&base_id).unwrap().into_subtitle();
                debug!("[sub:{}] Broadcasting", sub.id);
                state.subtitles.write().await.insert(sub.id, sub.clone());
                let _ = tx.send(sub);
            }
            continue;
        }

        // Handle subtitle property changes
        if json.get("event") == Some(&serde_json::json!("property-change"))
            && let Some(text) = json
                .get("data")
                .and_then(|d| d.as_str())
                .filter(|s| !s.is_empty())
        {
            let subtitle_id = next_subtitle_id;
            next_subtitle_id += 1;

            let base_id = next_request_id;
            next_request_id += 10;

            // Query all properties we need
            let cmd = format!(
                concat!(
                    "{{\"command\":[\"get_property\",\"sub-start\"],\"request_id\":{0}}}\n",
                    "{{\"command\":[\"get_property\",\"sub-end\"],\"request_id\":{1}}}\n",
                    "{{\"command\":[\"get_property\",\"path\"],\"request_id\":{2}}}\n",
                    "{{\"command\":[\"get_property\",\"aid\"],\"request_id\":{3}}}\n"
                ),
                base_id,
                base_id + 1,
                base_id + 2,
                base_id + 3
            );

            mpv.write_all(cmd.as_bytes()).await?;
            pending.insert(base_id, PendingSubtitle::new(subtitle_id, text.to_string()));
            info!("[sub:{}] {}", subtitle_id, text);
        }
    }
}

async fn handle_client(
    stream: TcpStream,
    id: u64,
    state: Arc<SharedState>,
    mut subtitle_rx: broadcast::Receiver<Subtitle>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws = accept_async(stream).await?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    loop {
        tokio::select! {
            Ok(sub) = subtitle_rx.recv() => {
                let msg = serde_json::json!({
                    "type": "subtitle",
                    "id": sub.id,
                    "subtitle": sub.text,
                    "sub_start": sub.sub_start,
                    "sub_end": sub.sub_end,
                });
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

async fn handle_request(text: &str, client_id: u64, state: &Arc<SharedState>) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let request_type = json.get("request")?.as_str()?;

    // Handle audio_range requests (multi-subtitle audio)
    if request_type == "audio_range" {
        let start_id = json.get("start_id")?.as_u64()?;
        let end_id = json.get("end_id")?.as_u64()?;

        let offset_start = json.get("offset_start").and_then(|v| v.as_f64());
        let offset_end = json.get("offset_end").and_then(|v| v.as_f64());
        let store = state.subtitles.read().await;
        let start = store.get(&start_id)?;
        let end = store.get(&end_id)?;
        let request = FfmpegRequest::audio_range(
            start.sub_start,
            end.sub_end,
            &start.media_path,
            start.aid,
            offset_start,
            offset_end,
        );
        drop(store);

        info!(
            "[client:{}] Requesting audio_range from subtitle {} to {}",
            client_id, start_id, end_id
        );

        let data = tokio::task::spawn_blocking(move || request.execute())
            .await
            .ok()?;

        return Some(
            serde_json::json!({
                "type": "audio_range",
                "start_id": start_id,
                "end_id": end_id,
                "data": data,
            })
            .to_string(),
        );
    }

    // Handle single-subtitle requests (thumbnail, audio)
    let subtitle_id = json.get("id")?.as_u64()?;
    let media_type = MediaType::from_str(request_type)?;
    let store = state.subtitles.read().await;
    let sub = store.get(&subtitle_id)?.clone();
    drop(store);

    let offset_start = json.get("offset_start").and_then(|v| v.as_f64());
    let offset_end = json.get("offset_end").and_then(|v| v.as_f64());
    let request = match media_type {
        MediaType::Thumbnail => FfmpegRequest::thumbnail(&sub),
        MediaType::Audio => FfmpegRequest::audio(&sub, offset_start, offset_end),
    };
    info!(
        "[client:{}] Requesting {} for subtitle {}",
        client_id, request_type, subtitle_id
    );

    let req_type = request_type.to_string();
    let data = tokio::task::spawn_blocking(move || request.execute())
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
