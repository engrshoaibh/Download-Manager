use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ExtensionStream {
    pub url: String,
    pub mime_type: String,
    pub content_length: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SessionHeaders {
    #[serde(rename = "User-Agent")]
    pub user_agent: String,
    #[serde(rename = "Referer")]
    pub referer: String,
    #[serde(rename = "Origin")]
    pub origin: String,
    #[serde(rename = "Cookie")]
    pub cookie: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NativeMessagePayload {
    pub task_id: String,
    pub title: String,
    pub video_stream: ExtensionStream,
    pub audio_stream: ExtensionStream,
    pub session_headers: SessionHeaders,
    pub output_path: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status")]
pub enum NativeMessageEvent {
    #[serde(rename = "NEED_REFRESH")]
    NeedRefresh {
        event: String, // "EXPIRED_URL"
        task_id: String,
        stream_type: String,
        last_known_offset: u64,
    },
    #[serde(rename = "DOWNLOADING")]
    Downloading {
        task_id: String,
        progress_percent: f64,
        download_speed_bps: u64,
    },
    #[serde(rename = "COMPLETED")]
    Completed {
        task_id: String,
        final_path: String,
    },
    #[serde(rename = "ERROR")]
    Error {
        task_id: String,
        error_code: String,
        message: String,
    },
}
