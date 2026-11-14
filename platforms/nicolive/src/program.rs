use std::sync::LazyLock;

use fake_user_agent::get_chrome_rua;
use regex::Regex;
use reqwest::{Client, ClientBuilder, header::SET_COOKIE};

static NICO_METADATA_REGEXP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<script id="embedded-data" data-props="([^"]+)""#).unwrap());

static NICO_SERVER_RESPONSE_REGEXP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<meta name="server-response" content="([^"]+)"#).unwrap());

#[derive(Debug)]
pub struct NicoEmbeddedData {
    client: Client,
    data: serde_json::Value,
}

impl NicoEmbeddedData {
    pub async fn new<S>(
        builder: ClientBuilder,
        live_url: S,
        user_session: Option<&str>,
    ) -> anyhow::Result<Self>
    where
        S: AsRef<str>,
    {
        let mut headers = reqwest::header::HeaderMap::new();
        let user_session = user_session.unwrap_or_default();
        headers.insert(
            reqwest::header::COOKIE,
            reqwest::header::HeaderValue::from_str(&format!("user_session={user_session}"))?,
        );
        let client = builder.default_headers(headers).build()?;

        let live_url = if live_url.as_ref().starts_with("lv") {
            &format!("https://live.nicovideo.jp/watch/{}", live_url.as_ref())
        } else {
            live_url.as_ref()
        };

        let response = client.get(live_url).send().await?;
        let text = response.text().await?;
        let json = NICO_METADATA_REGEXP
            .captures(&text)
            .and_then(|cap| cap.get(1))
            .map(|capture| {
                let capture = capture.as_str();
                // url decode
                html_escape::decode_html_entities(capture).to_string()
            })
            .unwrap();

        Ok(Self {
            client,
            data: serde_json::from_str(&json)?,
        })
    }

    pub async fn timeshift_reserve(&self) -> anyhow::Result<()> {
        let vid = self
            .data
            .get("program")
            .and_then(|program| program.get("nicoliveProgramId"))
            .and_then(|url| url.as_str())
            .unwrap();
        let url = format!("https://live2.nicovideo.jp/api/v2/programs/{vid}/timeshift/reservation");
        let _response = self.client.post(url).send().await?;

        Ok(())
    }

    pub fn websocket_url(&self) -> Option<String> {
        let url = self.raw_websocket_url()?;
        if let Some(frontend_id) = self.frontend_id() {
            let mut url = url::Url::parse(&url).ok()?;
            url.query_pairs_mut()
                .append_pair("frontend_id", &frontend_id.to_string());
            Some(url.to_string())
        } else {
            Some(url)
        }
    }

    fn raw_websocket_url(&self) -> Option<String> {
        self.data
            .get("site")
            .and_then(|site| site.get("relive"))
            .and_then(|relive| relive.get("webSocketUrl"))
            .and_then(|url| url.as_str())
            .and_then(|url| if url.is_empty() { None } else { Some(url) })
            .map(|url| url.to_string())
    }

    fn frontend_id(&self) -> Option<i64> {
        self.data
            .get("site")
            .and_then(|site| site.get("frontendId"))
            .and_then(|id| id.as_i64())
    }

    pub fn program_id(&self) -> String {
        self.data
            .get("program")
            .and_then(|program| program.get("nicoliveProgramId"))
            .and_then(|id| id.as_str())
            .map(|id| id.to_string())
            .unwrap()
    }

    pub fn program_title(&self) -> String {
        self.data
            .get("program")
            .and_then(|program| program.get("title"))
            .and_then(|title| title.as_str())
            .map(|title| title.to_string())
            .unwrap()
    }

    pub fn program_description(&self) -> String {
        self.data
            .get("program")
            .and_then(|program| program.get("description"))
            .and_then(|description| description.as_str())
            .map(|description| description.to_string())
            .unwrap()
    }

    pub fn program_end_time(&self) -> u64 {
        self.data
            .get("program")
            .and_then(|program| program.get("endTime"))
            .and_then(|end_at| end_at.as_u64())
            .unwrap()
    }

    pub fn audience_token(&self) -> anyhow::Result<String> {
        let wss_url = self
            .websocket_url()
            .ok_or_else(|| anyhow::anyhow!("no websocket url"))?;

        let (_, audience_token) = wss_url
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("can not extract audience token from url: {wss_url}"))?;

        Ok(audience_token.to_string())
    }

    pub fn best_quality(&self) -> anyhow::Result<String> {
        let quality = self
            .data
            .get("program")
            .and_then(|program| program.get("stream"))
            .and_then(|stream| stream.as_object())
            .and_then(|stream| stream.get("maxQuality"))
            .and_then(|quality| quality.as_str())
            .ok_or_else(|| anyhow::anyhow!("no max quality"))?;

        Ok(quality.to_string())
    }

    pub fn channel_id(&self) -> anyhow::Result<String> {
        let channel_id = self
            .data
            .get("socialGroup")
            .and_then(|site| site.get("id"))
            .and_then(|id| id.as_str())
            .ok_or_else(|| anyhow::anyhow!("no channel id"))?;

        Ok(channel_id.to_string())
    }

    pub fn channel_name(&self) -> anyhow::Result<String> {
        let channel_name = self
            .data
            .get("socialGroup")
            .and_then(|site| site.get("name"))
            .and_then(|name| name.as_str())
            .ok_or_else(|| anyhow::anyhow!("no channel name"))?;

        Ok(channel_name.to_string())
    }

    /// The viewer's account type reported by the watch page: `"premium"`,
    /// `"free"`, or `"non"` for requests without a valid user session.
    pub fn account_type(&self) -> Option<&str> {
        self.data
            .get("user")
            .and_then(|user| user.get("accountType"))
            .and_then(|account_type| account_type.as_str())
    }

    /// Whether the watch page reports a Niconico Premium account.
    ///
    /// Chase play is premium-only: the server rejects `chasePlay: true` from
    /// free and anonymous sessions, so callers must check this before asking
    /// for it.
    pub fn is_premium(&self) -> bool {
        self.account_type() == Some("premium")
    }
}

pub struct NivoServerResponse {
    client: Client,
    data: serde_json::Value,
}

impl NivoServerResponse {
    pub async fn new<S>(
        builder: ClientBuilder,
        video_url: S,
        user_session: Option<&str>,
    ) -> anyhow::Result<Self>
    where
        S: AsRef<str>,
    {
        let mut headers = reqwest::header::HeaderMap::new();
        let user_session = user_session.unwrap_or_default();
        headers.insert(
            reqwest::header::COOKIE,
            reqwest::header::HeaderValue::from_str(&format!("user_session={user_session}"))?,
        );
        let client = builder
            .default_headers(headers)
            .user_agent(get_chrome_rua())
            .build()?;

        let live_url = if video_url.as_ref().starts_with("so") {
            &format!("https://www.nicovideo.jp/watch/{}", video_url.as_ref())
        } else {
            video_url.as_ref()
        };

        let response = client.get(live_url).send().await?;
        let text = response.text().await?;
        let json = NICO_SERVER_RESPONSE_REGEXP
            .captures(&text)
            .and_then(|cap| cap.get(1))
            .map(|capture| {
                let capture = capture.as_str();
                // url decode
                html_escape::decode_html_entities(capture).to_string()
            })
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to extract server response from the web page")
            })?;

        Ok(Self {
            client,
            data: serde_json::from_str(&json)?,
        })
    }

    pub async fn playlist_url(&self) -> anyhow::Result<(String, String)> {
        let video_id = self
            .video_id()
            .ok_or_else(|| anyhow::anyhow!("no video id"))?;
        let url = format!("https://nvapi.nicovideo.jp/v1/watch/{video_id}/access-rights/hls",);

        let action_track_id = self
            .action_track_id()
            .ok_or_else(|| anyhow::anyhow!("no action track id"))?;
        let access_right_key = self
            .access_right_key()
            .ok_or_else(|| anyhow::anyhow!("no access right key"))?;

        let video_qualities = self
            .video_qualities()
            .ok_or_else(|| anyhow::anyhow!("no video quality"))?;
        let audio_qualities = self
            .audio_qualities()
            .ok_or_else(|| anyhow::anyhow!("no audio quality"))?;
        let mut outputs = Vec::with_capacity(video_qualities.len() * audio_qualities.len());
        for video_quality in video_qualities.iter() {
            for audio_quality in audio_qualities.iter() {
                outputs.push([video_quality.as_str(), audio_quality.as_str()]);
            }
        }

        let json = serde_json::json!({
            "outputs": outputs,
        });

        let response = self
            .client
            .post(url)
            .query(&[("actionTrackId", action_track_id)])
            .header("x-access-right-key", access_right_key)
            .header("x-frontend-id", "6")
            .header("x-frontend-version", "0")
            .header("x-niconico-language", "ja-JP")
            .header("x-request-with", "nicovideo")
            .json(&json)
            .send()
            .await?;

        let cookies = response.headers().get_all(SET_COOKIE);
        let cookies = cookies
            .into_iter()
            .filter_map(|cookie| {
                let Ok(cookie) = cookie.to_str() else {
                    return None;
                };
                let (kv, _) = cookie.split_once(';')?;
                Some(kv)
            })
            .collect::<Vec<_>>()
            .join("; ");

        let data: serde_json::Value = response.json().await?;
        let url = data["data"]["contentUrl"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no url"))?;

        Ok((url.to_string(), cookies))
    }

    fn response(&self) -> Option<&serde_json::Value> {
        self.data.get("data").and_then(|data| data.get("response"))
    }

    /// The object that carries watch metadata. Current watch pages nest the
    /// payload under `response.$watchV4.data`; older pages kept these fields
    /// directly on `response`.
    fn watch(&self) -> Option<&serde_json::Value> {
        let response = self.response()?;
        response
            .get("$watchV4")
            .and_then(|watch| watch.get("data"))
            .or(Some(response))
    }

    fn watch_media(&self) -> Option<&serde_json::Value> {
        self.watch().and_then(|watch| watch.get("media"))
    }

    pub fn program_title(&self) -> Option<String> {
        self.watch()
            .and_then(|watch| watch.get("video"))
            .and_then(|video| video.get("title"))
            .and_then(|title| title.as_str())
            .map(|title| title.to_string())
    }

    fn video_id(&self) -> Option<String> {
        self.watch()
            .and_then(|watch| watch.get("video"))
            .and_then(|video| video.get("id"))
            .and_then(|id| id.as_str())
            .map(|id| id.to_string())
    }

    fn access_right_key(&self) -> Option<String> {
        let media = self.watch_media()?;
        media
            .get("accessRightKey")
            .or_else(|| {
                media
                    .get("domand")
                    .and_then(|domand| domand.get("accessRightKey"))
            })
            .and_then(|key| key.as_str())
            .map(|key| key.to_string())
    }

    /// Playable quality ids for `videos` or `audios`. Current pages list them
    /// under `media.contents`; legacy pages under `media.domand`.
    fn qualities(&self, kind: &str) -> Option<Vec<String>> {
        let media = self.watch_media()?;
        let qualities = media
            .get("contents")
            .and_then(|contents| contents.get(kind))
            .or_else(|| {
                media
                    .get("domand")
                    .and_then(|domand| domand.get(kind))
            })?;
        qualities.as_array().and_then(|qualities| {
            qualities
                .iter()
                .filter(|quality| {
                    quality
                        .get("isAvailable")
                        .and_then(|available| available.as_bool())
                        .unwrap_or_default()
                })
                .map(|quality| quality.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect::<Option<Vec<_>>>()
        })
    }

    fn video_qualities(&self) -> Option<Vec<String>> {
        self.qualities("videos")
    }

    fn audio_qualities(&self) -> Option<Vec<String>> {
        self.qualities("audios")
    }

    fn action_track_id(&self) -> Option<String> {
        self.watch()
            .and_then(|watch| watch.get("client"))
            .and_then(|client| client.get("watchTrackId"))
            .and_then(|id| id.as_str())
            .map(|id| id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::program::NivoServerResponse;

    use super::NicoEmbeddedData;
    use serde_json::json;

    fn embedded_data(props: serde_json::Value) -> NicoEmbeddedData {
        NicoEmbeddedData {
            client: reqwest::Client::new(),
            data: props,
        }
    }

    fn server_response(props: serde_json::Value) -> NivoServerResponse {
        NivoServerResponse {
            client: reqwest::Client::new(),
            data: props,
        }
    }

    #[test]
    fn watch_v4_payload_exposes_video_fields() {
        let response = server_response(json!({
            "data": {
                "response": {
                    "$watchV4": {
                        "data": {
                            "video": { "id": "sm16550626", "title": "sample" },
                            "client": { "watchTrackId": "track-1" },
                            "media": {
                                "accessRightKey": "right-key",
                                "contents": {
                                    "videos": [
                                        { "id": "video-h264-360p", "isAvailable": true },
                                        { "id": "video-h264-1080p", "isAvailable": false }
                                    ],
                                    "audios": [
                                        { "id": "audio-aac-192kbps", "isAvailable": true }
                                    ]
                                }
                            }
                        }
                    }
                }
            }
        }));

        assert_eq!(response.video_id().as_deref(), Some("sm16550626"));
        assert_eq!(response.program_title().as_deref(), Some("sample"));
        assert_eq!(response.action_track_id().as_deref(), Some("track-1"));
        assert_eq!(response.access_right_key().as_deref(), Some("right-key"));
        assert_eq!(
            response.video_qualities(),
            Some(vec!["video-h264-360p".to_string()])
        );
        assert_eq!(
            response.audio_qualities(),
            Some(vec!["audio-aac-192kbps".to_string()])
        );
    }

    #[test]
    fn legacy_flat_payload_still_works() {
        let response = server_response(json!({
            "data": {
                "response": {
                    "video": { "id": "sm1", "title": "old" },
                    "client": { "watchTrackId": "track-0" },
                    "media": {
                        "domand": {
                            "accessRightKey": "old-key",
                            "videos": [{ "id": "video-old", "isAvailable": true }],
                            "audios": [{ "id": "audio-old", "isAvailable": true }]
                        }
                    }
                }
            }
        }));

        assert_eq!(response.video_id().as_deref(), Some("sm1"));
        assert_eq!(response.program_title().as_deref(), Some("old"));
        assert_eq!(response.action_track_id().as_deref(), Some("track-0"));
        assert_eq!(response.access_right_key().as_deref(), Some("old-key"));
        assert_eq!(
            response.video_qualities(),
            Some(vec!["video-old".to_string()])
        );
        assert_eq!(
            response.audio_qualities(),
            Some(vec!["audio-old".to_string()])
        );
    }

    #[test]
    fn account_type_identifies_premium_members() {
        assert!(embedded_data(json!({"user": {"accountType": "premium"}})).is_premium());
        assert!(!embedded_data(json!({"user": {"accountType": "free"}})).is_premium());
        assert!(!embedded_data(json!({"user": {"accountType": "non"}})).is_premium());
        let anonymous = embedded_data(json!({"user": {"isLoggedIn": false}}));
        assert_eq!(anonymous.account_type(), None);
        assert!(!anonymous.is_premium());
    }

    #[tokio::test]
    async fn test_get_live_info() -> anyhow::Result<()> {
        let data = NicoEmbeddedData::new(
            Default::default(),
            "https://live.nicovideo.jp/watch/lv347149115",
            None,
        )
        .await?;
        println!("{:?}", data.websocket_url());
        Ok(())
    }

    #[tokio::test]
    async fn test_get_playlist() -> anyhow::Result<()> {
        // 【Hatsune Miku】 livetune (kz) - Tell Your World - Full size Ver. 【Sound Radio】
        let data = NivoServerResponse::new(
            Default::default(),
            "https://www.nicovideo.jp/watch/sm16550626",
            None,
        )
        .await?;
        println!("{:?}", data.playlist_url().await?);
        Ok(())
    }
}
