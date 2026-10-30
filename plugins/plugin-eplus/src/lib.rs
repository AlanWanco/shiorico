use iori_eplus::{EplusClient, model::DeliveryStatus};
use shiori_plugin::*;

const ALLOW_PREPARING: bool = cfg!(shiorico_local_eplus);

pub struct EplusPlugin;

impl ShioriPlugin for EplusPlugin {
    fn name(&self) -> Cow<'static, str> {
        "eplus".into()
    }

    fn version(&self) -> Cow<'static, str> {
        "0.1.0".into()
    }

    fn description(&self) -> Option<Cow<'static, str>> {
        Some("Extracts eplus.jp live/archive stream playlists.".into())
    }

    fn arguments(&self, command: &mut dyn InspectorCommand) {
        command.add_argument(
            "eplus-username",
            Some("eplus_username"),
            "[Eplus] Your eplus.jp login ID (email).",
        );
        command.add_argument(
            "eplus-password",
            Some("eplus_password"),
            "[Eplus] Your eplus.jp login password.",
        );
        command.add_boolean_argument(
            "eplus-archive",
            "[Eplus] Download the archive/VOD stream only; combine with --wait to wait for it.",
        );
        command.add_argument(
            "eplus-refresh-interval",
            None,
            "[Eplus] Refresh interval in seconds for cookie updates. Mainly for debugging.",
        );
    }

    fn register(&self, registry: &mut dyn InspectorRegistry) -> anyhow::Result<()> {
        // Match eplus player pages: https://live.eplus.jp/ex/player?ib=...
        registry.register_inspector(
            Regex::new(r"https://live\.eplus\.jp/ex/player\?ib=(?P<ib>.+)").unwrap(),
            Box::new(EplusInspector),
            PriorityHint::Normal,
        );
        // Match vp pages: https://live.eplus.jp/vp/<id>
        registry.register_inspector(
            Regex::new(r"https://live\.eplus\.jp/vp/(?P<id>[^/?#]+)$").unwrap(),
            Box::new(EplusInspector),
            PriorityHint::Normal,
        );
        // Match direct event page URLs
        registry.register_inspector(
            Regex::new(r"https://live\.eplus\.jp/(?P<path>[^/]+)$").unwrap(),
            Box::new(EplusInspector),
            PriorityHint::Normal,
        );

        Ok(())
    }
}

struct EplusInspector;

#[async_trait]
impl Inspect for EplusInspector {
    fn name(&self) -> Cow<'static, str> {
        "eplus".into()
    }

    async fn inspect(
        &self,
        context: &ShioriContext,
        url: &str,
        _captures: &Captures,
        args: &dyn InspectorArguments,
    ) -> anyhow::Result<InspectResult> {
        let username = args.get_string("eplus-username");
        let password = args.get_string("eplus-password");

        // Create client using the shared IoriHttp cookie store.
        // After login, session cookies are stored in context.http's cookie jar.
        let client = match (username, password) {
            (Some(user), Some(pass)) => {
                EplusClient::login(context.http.builder(), url, &user, &pass).await?
            }
            _ => {
                log::info!("No eplus credentials provided, attempting anonymous access.");
                EplusClient::new(context.http.builder())?
            }
        };

        // Fetch event data — CloudFront Set-Cookie headers are stored in the shared jar.
        let event_data = client.get_event_data(url).await?;

        if event_data.delivery_status == DeliveryStatus::Preparing && !ALLOW_PREPARING {
            log::warn!(
                "Eplus event is PREPARING; no download will start until the event is ready."
            );
            return Ok(InspectResult::None);
        }

        if matches!(
            event_data.delivery_status,
            DeliveryStatus::Stopped | DeliveryStatus::WaitConfirmArchived
        ) {
            log::info!("Eplus event has ended; waiting for the archive to be published.");
            return Ok(InspectResult::None);
        }

        if event_data.m3u8_urls.is_empty() {
            log::warn!("No m3u8 URLs found on the page.");
            return Ok(InspectResult::None);
        }

        let archive_only = args.get_boolean("eplus-archive")
            || event_data.delivery_status == DeliveryStatus::ConfirmedArchive;
        let Some(playlist_url) = client
            .select_best_playlist(&event_data.m3u8_urls, archive_only)
            .await
        else {
            log::info!("Title: {}", event_data.title);
            log::info!("Status: {:?}", event_data.delivery_status);
            log::info!("Playlist Candidates: {}", event_data.m3u8_urls.len());
            log::warn!("Could not determine a valid playlist URL.");
            return Ok(InspectResult::None);
        };

        if event_data.delivery_status == DeliveryStatus::Preparing {
            log::info!(
                "Event is marked PREPARING, but a cookie-authenticated playlist is already available."
            );
        }

        // Keep the original CloudFront Set-Cookie attributes. In particular, the
        // Path is bound to /out/v1/<playlist-id>; flattening these cookies to
        // name=value here would create a stale host-only duplicate on download.
        // Also include session cookies needed for the event-page refresh task.
        let mut cookies = event_data.cloudfront_cookies.clone();
        let playlist_cookies = context.http.export_cookies_for_url(&playlist_url);
        let event_cookies = context.http.export_cookies_for_url(url);
        for cookie in playlist_cookies.into_iter().chain(event_cookies) {
            let already_present = cookie.split_once('=').is_some_and(|(name, _)| {
                cookies.iter().any(|existing| {
                    existing
                        .split_once('=')
                        .is_some_and(|(existing_name, _)| existing_name.trim() == name.trim())
                })
            });
            if !already_present {
                cookies.push(cookie);
            }
        }

        let content_type = if EplusClient::is_archive_playlist(&playlist_url) {
            ContentType::Archive
        } else {
            ContentType::Live
        };

        Ok(InspectResult::Playlist(InspectPlaylist {
            title: Some(event_data.title),
            playlist_url,
            playlist_type: PlaylistType::HLS,
            cookies,
            source: Some(
                InspectSource::new("eplus", content_type)
                    .with_content_id(event_data.app_id)
                    .with_original_url(url),
            ),
            ..Default::default()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{ALLOW_PREPARING, EplusInspector};
    use shiori_plugin::{ContentType, Inspect, InspectResult, InspectorArguments, ShioriContext};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const PLAYLIST: &str = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n";

    struct Arguments {
        archive_only: bool,
    }

    impl InspectorArguments for Arguments {
        fn get_string(&self, _: &'static str) -> Option<String> {
            None
        }

        fn get_boolean(&self, argument: &'static str) -> bool {
            argument == "eplus-archive" && self.archive_only
        }
    }

    struct Fixture {
        context: ShioriContext,
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Fixture {
        async fn start(status: &str, archive: Option<(u16, &'static str)>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let live = format!("http://stream.live.eplus.jp:{}/live.m3u8", address.port());
            let vod = format!("http://vod.live.eplus.jp:{}/archive.m3u8", address.port());
            let channels = if archive.is_some() {
                format!("[\"{live}\",\"{vod}\"]")
            } else {
                format!("[\"{live}\"]")
            };
            let page = format!(
                "<script>var app = {{\"app_id\":\"fixture\",\"app_name\":\"Fixture\",\"delivery_status\":\"{status}\",\"archive_mode\":\"ON\"}};</script>\nvar listChannels = {channels};"
            );
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded_requests = requests.clone();
            let server = tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                        let mut buffer = [0; 1024];
                        let length = stream.read(&mut buffer).await.unwrap();
                        if length == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..length]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let path = request.split_whitespace().nth(1).unwrap();
                    recorded_requests.lock().unwrap().push(path.to_string());
                    let (status, body) = match path {
                        "/event" => (200, page.as_str()),
                        "/live.m3u8" => (200, PLAYLIST),
                        "/archive.m3u8" => archive.unwrap_or((404, "Not found")),
                        _ => (404, "Not found"),
                    };
                    let reason = if status == 200 { "OK" } else { "Not Found" };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
            });
            let context = ShioriContext::new(shiori_plugin::iori::IoriHttp::new(move || {
                reqwest::Client::builder()
                    .no_proxy()
                    .resolve("stream.live.eplus.jp", address)
                    .resolve("vod.live.eplus.jp", address)
            }));
            Self {
                context,
                url: format!("http://{address}/event"),
                requests,
                server,
            }
        }

        async fn inspect(&self, archive_only: bool) -> InspectResult {
            let regex = shiori_plugin::Regex::new(".*").unwrap();
            let captures = regex.captures(&self.url).unwrap();
            EplusInspector
                .inspect(
                    &self.context,
                    &self.url,
                    &captures,
                    &Arguments { archive_only },
                )
                .await
                .unwrap()
        }

        fn requests_to(&self, path: &str) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| *request == path)
                .count()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    fn assert_playlist(result: InspectResult, content_type: ContentType, path: &str) {
        let InspectResult::Playlist(playlist) = result else {
            panic!("Expected a playable playlist");
        };
        assert!(playlist.playlist_url.ends_with(path));
        assert_eq!(playlist.source.unwrap().content_type, content_type);
    }

    #[tokio::test]
    async fn preparing_event_only_probes_playlist_in_local_build() {
        let fixture = Fixture::start("PREPARING", None).await;
        let result = fixture.inspect(false).await;
        if ALLOW_PREPARING {
            assert_playlist(result, ContentType::Live, "/live.m3u8");
            assert_eq!(fixture.requests_to("/live.m3u8"), 1);
        } else {
            assert!(matches!(result, InspectResult::None));
            assert_eq!(fixture.requests_to("/live.m3u8"), 0);
        }
    }

    #[tokio::test]
    async fn live_events_keep_the_live_playlist() {
        let fixture = Fixture::start("STARTED", Some((200, PLAYLIST))).await;
        assert_playlist(
            fixture.inspect(false).await,
            ContentType::Live,
            "/live.m3u8",
        );
        assert_eq!(fixture.requests_to("/archive.m3u8"), 0);
    }

    #[tokio::test]
    async fn ended_events_wait_for_archive_publication() {
        for status in ["STOPPED", "WAIT_CONFIRM_ARCHIVED"] {
            let fixture = Fixture::start(status, Some((200, PLAYLIST))).await;
            for archive_only in [false, true] {
                assert!(matches!(
                    fixture.inspect(archive_only).await,
                    InspectResult::None
                ));
            }
            assert_eq!(fixture.requests_to("/live.m3u8"), 0, "{status}");
            assert_eq!(fixture.requests_to("/archive.m3u8"), 0, "{status}");
        }
    }

    #[tokio::test]
    async fn archived_events_select_vod_without_probing_old_live_streams() {
        let fixture = Fixture::start("CONFIRMED_ARCHIVE", Some((200, PLAYLIST))).await;
        assert_playlist(
            fixture.inspect(false).await,
            ContentType::Archive,
            "/archive.m3u8",
        );
        assert_eq!(fixture.requests_to("/live.m3u8"), 0);
    }

    #[tokio::test]
    async fn unavailable_archives_do_not_fall_back_to_old_live_streams() {
        for response in [
            None,
            Some((404, "Not found")),
            Some((200, "<html>Not ready</html>")),
        ] {
            let fixture = Fixture::start("CONFIRMED_ARCHIVE", response).await;
            assert!(matches!(fixture.inspect(false).await, InspectResult::None));
            assert_eq!(fixture.requests_to("/live.m3u8"), 0);
        }
    }

    #[tokio::test]
    async fn archive_option_selects_vod_even_while_live_is_available() {
        let fixture = Fixture::start("STARTED", Some((200, PLAYLIST))).await;
        assert_playlist(
            fixture.inspect(true).await,
            ContentType::Archive,
            "/archive.m3u8",
        );
        assert_eq!(fixture.requests_to("/live.m3u8"), 0);
    }

    #[tokio::test]
    async fn archive_option_waits_instead_of_downloading_live() {
        for response in [
            None,
            Some((404, "Not found")),
            Some((200, "<html>Not ready</html>")),
        ] {
            let fixture = Fixture::start("STARTED", response).await;
            assert!(matches!(fixture.inspect(true).await, InspectResult::None));
            assert_eq!(fixture.requests_to("/live.m3u8"), 0);
        }
    }
}
