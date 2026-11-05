use reqwest::Client;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct IoriContext {
    pub client: Client,
    pub shaka_packager_command: Arc<Option<PathBuf>>,

    pub manifest_retries: u32,
    pub segment_retries: u32,

    /// Synthesize and try segments before the first media sequence of a live
    /// playlist.
    ///
    /// This only affects HLS sources whose segment names follow the media
    /// sequence; see `HlsMediaPlaylistSource::load_segments`. It backs the
    /// backfill cursor, so it is off unless the downloader runs in dual
    /// cursor mode.
    pub probe_backfill: bool,
}

impl Default for IoriContext {
    fn default() -> Self {
        Self {
            client: Client::new(),
            shaka_packager_command: Arc::new(None),
            manifest_retries: 3,
            segment_retries: 5,
            probe_backfill: false,
        }
    }
}
