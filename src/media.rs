use base64::Engine;
use log::{debug, warn};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::{env, fs, path::PathBuf};
use uuid::Uuid;

use crate::event_loop::Subtitle;

const DEFAULT_AUDIO_OFFSET: f64 = 0.25;

static FFMPEG_PATH: OnceLock<String> = OnceLock::new();

pub fn init_ffmpeg_path(path: &str) {
    let resolved = resolve_ffmpeg_path(path);
    if resolved != path {
        debug!("[media] Resolved ffmpeg '{}' -> '{}'", path, resolved);
    }
    FFMPEG_PATH.set(resolved).ok();
}

fn ffmpeg() -> &'static str {
    FFMPEG_PATH.get().map(|s| s.as_str()).unwrap_or("ffmpeg")
}

fn resolve_ffmpeg_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return "ffmpeg".to_string();
    }

    #[cfg(target_os = "macos")]
    {
        if trimmed == "ffmpeg" {
            for candidate in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"]
            {
                if fs::metadata(candidate).is_ok() {
                    return candidate.to_string();
                }
            }
        }
    }

    trimmed.to_string()
}

fn temp_path(prefix: &str, ext: &str) -> PathBuf {
    env::temp_dir().join(format!("{}_{}.{}", prefix, Uuid::new_v4(), ext))
}

#[derive(Debug, Clone, Copy)]
pub enum MediaType {
    Thumbnail,
    Audio,
}

impl MediaType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "thumbnail" => Some(Self::Thumbnail),
            "audio" => Some(Self::Audio),
            _ => None,
        }
    }
}

pub struct FfmpegRequest {
    output_path: PathBuf,
    args: Vec<String>,
}

impl FfmpegRequest {
    pub fn thumbnail(sub: &Subtitle) -> Self {
        let output = temp_path("thumb", "jpg");
        let mid_time = (sub.sub_start + sub.sub_end) / 2.0;

        debug!(
            "[media] Thumbnail at {:.3} from {}",
            mid_time, sub.media_path
        );

        Self {
            args: vec![
                "-ss".into(),
                format!("{:.3}", mid_time),
                "-i".into(),
                sub.media_path.clone(),
                "-vframes".into(),
                "1".into(),
                "-vf".into(),
                "scale=640:-2".into(),
                "-q:v".into(),
                "5".into(),
                "-y".into(),
                output.display().to_string(),
            ],
            output_path: output,
        }
    }

    pub fn audio(sub: &Subtitle, offset_start: Option<f64>, offset_end: Option<f64>) -> Self {
        Self::audio_range(
            sub.sub_start,
            sub.sub_end,
            &sub.media_path,
            sub.aid,
            offset_start,
            offset_end,
        )
    }

    pub fn audio_range(
        sub_start: f64,
        sub_end: f64,
        media_path: &str,
        aid: i64,
        offset_start: Option<f64>,
        offset_end: Option<f64>,
    ) -> Self {
        let output = temp_path("audio", "opus");
        let start_offset = offset_start.unwrap_or(DEFAULT_AUDIO_OFFSET);
        let end_offset = offset_end.unwrap_or(DEFAULT_AUDIO_OFFSET);
        let start = (sub_start - start_offset).max(0.0);
        let duration = sub_end - sub_start + start_offset + end_offset;

        debug!(
            "[media] Audio {:.3}-{:.3} from {}",
            start,
            start + duration,
            media_path
        );

        Self {
            args: vec![
                "-ss".into(),
                format!("{:.3}", start),
                "-i".into(),
                media_path.to_string(),
                "-t".into(),
                format!("{:.3}", duration),
                "-map".into(),
                format!("0:a:{}", (aid - 1).max(0)),
                "-vn".into(),
                "-ac".into(),
                "2".into(),
                "-b:a".into(),
                "128k".into(),
                "-y".into(),
                output.display().to_string(),
            ],
            output_path: output,
        }
    }

    pub fn from_type(media_type: MediaType, sub: &Subtitle) -> Self {
        match media_type {
            MediaType::Thumbnail => Self::thumbnail(sub),
            MediaType::Audio => Self::audio(sub, None, None),
        }
    }

    pub fn execute(self) -> Option<String> {
        debug!("[media] Running: {} {:?}", ffmpeg(), self.args);

        let result = Command::new(ffmpeg())
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();

        let cleanup = || {
            let _ = fs::remove_file(&self.output_path);
        };

        match result {
            Ok(out) if out.status.success() => match fs::read(&self.output_path) {
                Ok(data) if !data.is_empty() => {
                    cleanup();
                    Some(base64::engine::general_purpose::STANDARD.encode(&data))
                }
                _ => {
                    cleanup();
                    None
                }
            },
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let errors: Vec<_> = stderr.lines().rev().take(10).collect();
                warn!(
                    "[media] ffmpeg failed ({}): {}",
                    out.status,
                    errors.into_iter().rev().collect::<Vec<_>>().join(" | ")
                );
                cleanup();
                None
            }
            Err(e) => {
                warn!("[media] ffmpeg failed to start: {}", e);
                cleanup();
                None
            }
        }
    }
}
