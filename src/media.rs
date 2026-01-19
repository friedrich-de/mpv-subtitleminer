use base64::Engine;
use log::{debug, warn};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::{env, fs, path::PathBuf};
use uuid::Uuid;

use crate::event_loop::Subtitle;

const AUDIO_PADDING: f64 = 0.25;

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

#[derive(Debug, Clone, Copy)]
pub enum ImageFormat {
    Jpeg,
    Webp,
    Avif,
}

impl ImageFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            "avif" => Some(Self::Avif),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Avif => "avif",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageOptions {
    pub format: ImageFormat,
    pub quality: Option<u8>,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            format: ImageFormat::Jpeg,
            quality: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum AudioFormat {
    Mp3,
    Opus,
}

impl AudioFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "mp3" => Some(Self::Mp3),
            "opus" => Some(Self::Opus),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Opus => "audio/opus",
        }
    }

    pub fn default_bitrate(self) -> &'static str {
        match self {
            Self::Mp3 => "128k",
            Self::Opus => "96k",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioOptions {
    pub format: AudioFormat,
    pub bitrate: Option<String>,
    pub filters: Option<String>,
}

impl Default for AudioOptions {
    fn default() -> Self {
        Self {
            format: AudioFormat::Mp3,
            bitrate: None,
            filters: None,
        }
    }
}

pub struct FfmpegRequest {
    output_path: PathBuf,
    args: Vec<String>,
    ext: String,
    mime: String,
}

impl FfmpegRequest {
    pub fn thumbnail(sub: &Subtitle, options: ImageOptions) -> Self {
        let output = temp_path("thumb", options.format.ext());
        let mid_time = (sub.sub_start + sub.sub_end) / 2.0;

        debug!(
            "[media] Thumbnail at {:.3} from {}",
            mid_time, sub.media_path
        );

        let mut args = vec![
            "-ss".into(),
            format!("{:.3}", mid_time),
            "-i".into(),
            sub.media_path.clone(),
            "-vframes".into(),
            "1".into(),
            "-vf".into(),
            "scale=640:-2".into(),
        ];

        match options.format {
            ImageFormat::Jpeg => {
                args.extend(["-q:v".into(), "5".into()]);
            }
            ImageFormat::Webp => {
                let quality = options.quality.unwrap_or(80);
                args.extend([
                    "-c:v".into(),
                    "libwebp".into(),
                    "-quality".into(),
                    quality.to_string(),
                ]);
            }
            ImageFormat::Avif => {
                args.extend([
                    "-c:v".into(),
                    "libaom-av1".into(),
                    "-still-picture".into(),
                    "1".into(),
                    "-crf".into(),
                    "35".into(),
                    "-b:v".into(),
                    "0".into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                    "-f".into(),
                    "avif".into(),
                ]);
            }
        }

        args.extend(["-y".into(), output.display().to_string()]);

        Self {
            args,
            output_path: output,
            ext: options.format.ext().to_string(),
            mime: options.format.mime().to_string(),
        }
    }

    pub fn audio(sub: &Subtitle, options: AudioOptions) -> Self {
        Self::audio_range(
            sub.sub_start,
            sub.sub_end,
            &sub.media_path,
            sub.aid,
            options,
        )
    }

    pub fn audio_range(
        sub_start: f64,
        sub_end: f64,
        media_path: &str,
        aid: i64,
        options: AudioOptions,
    ) -> Self {
        let output = temp_path("audio", options.format.ext());
        let start = (sub_start - AUDIO_PADDING).max(0.0);
        let duration = sub_end - sub_start + AUDIO_PADDING * 2.0;

        debug!(
            "[media] Audio {:.3}-{:.3} from {}",
            start,
            start + duration,
            media_path
        );

        let mut args = vec![
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
        ];

        if let Some(filters) = options.filters.as_ref().filter(|value| !value.trim().is_empty()) {
            args.extend(["-af".into(), filters.to_string()]);
        }

        match options.format {
            AudioFormat::Mp3 => {
                args.extend(["-c:a".into(), "libmp3lame".into()]);
            }
            AudioFormat::Opus => {
                args.extend(["-c:a".into(), "libopus".into(), "-f".into(), "ogg".into()]);
            }
        }

        let bitrate = options
            .bitrate
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(options.format.default_bitrate());

        args.extend(["-b:a".into(), bitrate.to_string()]);
        args.extend(["-y".into(), output.display().to_string()]);

        Self {
            args,
            output_path: output,
            ext: options.format.ext().to_string(),
            mime: options.format.mime().to_string(),
        }
    }

    pub fn from_type(
        media_type: MediaType,
        sub: &Subtitle,
        image: ImageOptions,
        audio: AudioOptions,
    ) -> Self {
        match media_type {
            MediaType::Thumbnail => Self::thumbnail(sub, image),
            MediaType::Audio => Self::audio(sub, audio),
        }
    }

    pub fn ext(&self) -> &str {
        &self.ext
    }

    pub fn mime(&self) -> &str {
        &self.mime
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
