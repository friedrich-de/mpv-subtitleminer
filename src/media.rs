use base64::Engine;
use log::{debug, info, warn};
use serde::Deserialize;
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
            for candidate in [
                "/opt/homebrew/bin/ffmpeg",
                "/usr/local/bin/ffmpeg",
                "/usr/bin/ffmpeg",
            ] {
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

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ImageConfig {
    pub format: String,
    pub quality: i32,
    pub is_animated: bool,
    pub size: Option<String>,
    pub advanced_args: Option<String>,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            format: "jpeg".to_string(),
            quality: 5,
            is_animated: false,
            size: None,
            advanced_args: None,
        }
    }
}

impl ImageConfig {
    pub fn get_extension(&self) -> &str {
        let fmt = self.format.trim_start_matches('.');
        if fmt.is_empty() {
            return "jpg";
        }
        match fmt {
            "jpeg" | "jpg" => "jpg",
            "avif" | "avif_animated" => "avif",
            "webp" | "webp_animated" => "webp",
            other => other,
        }
    }

    pub fn apply_to_args(&self, args: &mut Vec<String>, sub: &Subtitle) {
        if let Some(advanced) = &self.advanced_args {
            if self.is_animated {
                args.extend(["-t".into(), format!("{:.3}", sub.sub_end - sub.sub_start)]);
            } else {
                args.extend(["-vframes".into(), "1".into()]);
            }
            args.extend(advanced.split_whitespace().map(|s| s.to_string()));
            return;
        }

        if self.is_animated {
            args.extend(["-t".into(), format!("{:.3}", sub.sub_end - sub.sub_start)]);
        } else {
            args.extend(["-vframes".into(), "1".into()]);
        }

        if let Some(size) = &self.size {
            if !size.trim().is_empty() {
                args.extend(["-vf".into(), format!("scale={}", size)]);
            }
        }

        match self.format.as_str() {
            "jpeg" | "jpg" => {
                args.extend([
                    "-c:v".into(),
                    "mjpeg".into(),
                    "-q:v".into(),
                    format!("{}", self.quality.clamp(1, 31)),
                ]);
            }
            "avif" => {
                args.extend([
                    "-c:v".into(),
                    "libaom-av1".into(),
                    "-crf".into(),
                    format!("{}", self.quality.clamp(0, 63)),
                    "-cpu-used".into(),
                    "8".into(),
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                ]);
                if !self.is_animated {
                    args.extend(["-still-picture".into(), "1".into()]);
                }
            }
            _ => {
                args.extend([
                    "-c:v".into(),
                    "libwebp".into(),
                    "-quality".into(),
                    format!("{}", self.quality.clamp(0, 100)),
                ]);
                if self.is_animated {
                    args.extend(["-loop".into(), "0".into()]);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub format: String,
    pub quality: i32,
    pub filters: Option<String>,
    pub advanced_args: Option<String>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            format: "mp3".to_string(),
            quality: 128,
            filters: None,
            advanced_args: None,
        }
    }
}

impl AudioConfig {
    pub fn get_extension(&self) -> &str {
        let fmt = self.format.trim_start_matches('.');
        if fmt.is_empty() {
            return "mp3";
        }
        match fmt {
            "mp3" => "mp3",
            "opus" => "opus",
            other => other,
        }
    }

    pub fn apply_to_args(&self, args: &mut Vec<String>) {
        if let Some(advanced) = &self.advanced_args {
            args.extend(advanced.split_whitespace().map(|s| s.to_string()));
            return;
        }

        if self.format == "mp3" {
            args.extend([
                "-c:a".into(),
                "libmp3lame".into(),
                "-b:a".into(),
                format!("{}k", self.quality.clamp(8, 320)),
            ]);
        } else {
            args.extend([
                "-c:a".into(),
                "libopus".into(),
                "-b:a".into(),
                format!("{}k", self.quality.clamp(8, 512)),
            ]);
        }

        let mut filters = vec!["afade=t=in:d=0.005".to_string()];
        if let Some(f) = &self.filters {
            if !f.trim().is_empty() {
                filters.push(f.clone());
            }
        }
        args.extend(["-af".into(), filters.join(",")]);
    }
}

#[derive(Debug, Clone)]
pub struct FfmpegRequest {
    output_path: PathBuf,
    args: Vec<String>,
}

impl FfmpegRequest {
    pub fn thumbnail(sub: &Subtitle, config: Option<ImageConfig>) -> Self {
        let config = config.unwrap_or_default();
        let is_animated = config.is_animated;

        let ext = config.get_extension();
        let output = temp_path("thumb", ext);
        let mid_time = (sub.sub_start + sub.sub_end) / 2.0;

        debug!(
            "[media] Thumbnail ({}) at {:.3} from {}",
            config.format, mid_time, sub.media_path
        );

        let ss = if is_animated { sub.sub_start } else { mid_time };

        let mut args = vec![
            "-ss".into(),
            format!("{:.3}", ss),
            "-i".into(),
            sub.media_path.clone(),
        ];

        config.apply_to_args(&mut args, sub);

        args.extend(["-y".into(), output.display().to_string()]);
        Self {
            args,
            output_path: output,
        }
    }

    pub fn audio(
        sub: &Subtitle,
        offset_start: Option<f64>,
        offset_end: Option<f64>,
        config: Option<AudioConfig>,
    ) -> Self {
        Self::audio_range(
            sub.sub_start,
            sub.sub_end,
            &sub.media_path,
            sub.aid,
            offset_start,
            offset_end,
            config,
        )
    }

    pub fn audio_range(
        sub_start: f64,
        sub_end: f64,
        media_path: &str,
        aid: i64,
        offset_start: Option<f64>,
        offset_end: Option<f64>,
        config: Option<AudioConfig>,
    ) -> Self {
        let config = config.unwrap_or_default();
        let ext = config.get_extension();
        let output = temp_path("audio", ext);
        let start_offset = offset_start.unwrap_or(DEFAULT_AUDIO_OFFSET);
        let end_offset = offset_end.unwrap_or(DEFAULT_AUDIO_OFFSET);
        let start = (sub_start - start_offset).max(0.0);
        let duration = sub_end - sub_start + start_offset + end_offset;

        debug!(
            "[media] Audio ({}) {:.3}-{:.3} from {}",
            config.format,
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
        ];

        config.apply_to_args(&mut args);

        args.extend(["-y".into(), output.display().to_string()]);

        Self {
            args,
            output_path: output,
        }
    }

    pub fn execute(self) -> Option<String> {
        info!("[media] Running: {} {}", ffmpeg(), self.args.join(" "));

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
                    warn!(
                        "[media] ffmpeg succeeded but output file is empty or missing: {}",
                        self.output_path.display()
                    );
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
