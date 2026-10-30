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
            "[Eplus] Prefer archive/VOD stream over live stream.",
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
        let prefer_archive = args.get_boolean("eplus-archive");

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

        if event_data.m3u8_urls.is_empty() {
            log::warn!("No m3u8 URLs found on the page.");
            return Ok(InspectResult::None);
        }

        // Select the best playlist URL
        let Some(playlist_url) = client
            .select_best_playlist(&event_data.m3u8_urls, prefer_archive)
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

        let content_type = if playlist_url.contains("stream.live.eplus.jp") {
            ContentType::Live
        } else {
            ContentType::Archive
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
    use shiori_plugin::{Inspect, InspectResult, InspectorArguments, ShioriContext};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct AnonymousArguments;

    impl InspectorArguments for AnonymousArguments {
        fn get_string(&self, _: &'static str) -> Option<String> {
            None
        }

        fn get_boolean(&self, _: &'static str) -> bool {
            false
        }
    }

    async fn inspect_event(status: &str) -> (InspectResult, usize) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let host = "stream.live.eplus.jp";
        let playlist = format!("http://{host}:{}/prepared.m3u8", address.port());
        let page = format!(
            "<script>var app = {{\"appId\":\"fixture\",\"appName\":\"Fixture\",\"deliveryStatus\":\"{status}\"}};</script>\nvar listChannels = [\"{playlist}\"];"
        );
        let server = tokio::spawn(async move {
            let mut playlist_requests = 0;
            loop {
                let accepted =
                    tokio::time::timeout(std::time::Duration::from_millis(250), listener.accept())
                        .await;
                let Ok(Ok((mut stream, _))) = accepted else {
                    break;
                };
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut buffer = [0; 1024];
                    let length = stream.read(&mut buffer).await.unwrap();
                    if length == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..length]);
                }
                let event = request.starts_with(b"GET /event ");
                if !event {
                    playlist_requests += 1;
                }
                let body = if event {
                    page.as_str()
                } else {
                    "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            playlist_requests
        });
        let context = ShioriContext::new(shiori_plugin::iori::IoriHttp::new(move || {
            reqwest::Client::builder().no_proxy().resolve(host, address)
        }));
        let url = format!("http://{address}/event");
        let regex = shiori_plugin::Regex::new(".*").unwrap();
        let captures = regex.captures(&url).unwrap();
        let result = EplusInspector
            .inspect(&context, &url, &captures, &AnonymousArguments)
            .await
            .unwrap();
        (result, server.await.unwrap())
    }

    #[tokio::test]
    async fn preparing_event_only_probes_playlist_in_local_build() {
        let (result, requests) = inspect_event("PREPARING").await;
        if ALLOW_PREPARING {
            assert!(matches!(result, InspectResult::Playlist(_)));
            assert_eq!(requests, 1);
        } else {
            assert!(matches!(result, InspectResult::None));
            assert_eq!(requests, 0);
        }
    }

    #[tokio::test]
    async fn started_and_archived_events_still_probe_the_playlist() {
        for status in ["STARTED", "CONFIRMED_ARCHIVE"] {
            let (result, requests) = inspect_event(status).await;
            assert!(matches!(result, InspectResult::Playlist(_)), "{status}");
            assert_eq!(requests, 1, "{status}");
        }
    }
}
