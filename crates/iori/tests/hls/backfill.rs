use crate::hls::setup_mock_server;
use iori::{
    context::IoriContext,
    hls::{HlsPlaylistSource, segment::M3u8Segment},
};

const LIVE_PLAYLIST: &str = "#EXTM3U
#EXT-X-TARGETDURATION:10
#EXT-X-VERSION:3
#EXT-X-MEDIA-SEQUENCE:5
#EXTINF:9.009,
show_0_720p_00005.ts
#EXTINF:9.009,
show_0_720p_00006.ts
#EXTINF:9.009,
show_0_720p_00007.ts";

async fn load(playlist: &str, probe_backfill: bool) -> anyhow::Result<Vec<M3u8Segment>> {
    let (uri, _server) = setup_mock_server(playlist).await;
    let context = IoriContext {
        probe_backfill,
        ..IoriContext::default()
    };
    let mut source = HlsPlaylistSource::new(uri.parse()?, None);

    let latest_media_sequences = source.load_streams(&context).await?;
    let (mut streams, _is_end) = source
        .load_segments(&context, &latest_media_sequences)
        .await?;
    Ok(streams.remove(0))
}

#[tokio::test]
async fn probes_older_segments_before_the_window() -> anyhow::Result<()> {
    let segments = load(LIVE_PLAYLIST, true).await?;

    assert_eq!(
        segments.len(),
        8,
        "5 probes followed by the 3 window segments"
    );

    for (index, segment) in segments.iter().take(5).enumerate() {
        assert!(segment.is_probe);
        assert_eq!(segment.media_sequence, index as u64);
        assert_eq!(segment.sequence, index as u64);
        assert_eq!(segment.filename, format!("show_0_720p_{index:05}.ts"));
        assert!(
            segment
                .url
                .as_str()
                .ends_with(&format!("/show_0_720p_{index:05}.ts")),
            "unexpected probe url {}",
            segment.url
        );
    }

    for (offset, segment) in segments[5..].iter().enumerate() {
        assert!(!segment.is_probe);
        assert_eq!(segment.media_sequence, 5 + offset as u64);
        assert_eq!(segment.sequence, 5 + offset as u64);
        assert_eq!(
            segment.filename,
            format!("show_0_720p_{:05}.ts", 5 + offset)
        );
    }

    Ok(())
}

#[tokio::test]
async fn probes_are_off_without_the_flag() -> anyhow::Result<()> {
    let segments = load(LIVE_PLAYLIST, false).await?;

    assert_eq!(segments.len(), 3);
    assert!(segments.iter().all(|segment| !segment.is_probe));
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.sequence)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    Ok(())
}

#[tokio::test]
async fn names_not_matching_the_media_sequence_are_not_probed() -> anyhow::Result<()> {
    // The trailing number is 90005, not the media sequence, so the older names
    // cannot be derived from it.
    let playlist = LIVE_PLAYLIST.replace("show_0_720p_0000", "show_0_720p_9");
    let segments = load(&playlist, true).await?;

    assert_eq!(segments.len(), 3);
    assert!(segments.iter().all(|segment| !segment.is_probe));

    Ok(())
}

#[tokio::test]
async fn a_vod_playlist_is_not_probed() -> anyhow::Result<()> {
    let playlist = format!("{LIVE_PLAYLIST}\n#EXT-X-ENDLIST");
    let segments = load(&playlist, true).await?;

    assert_eq!(segments.len(), 3);
    assert!(segments.iter().all(|segment| !segment.is_probe));

    Ok(())
}

#[tokio::test]
async fn fmp4_segments_are_not_probed() -> anyhow::Result<()> {
    let playlist = LIVE_PLAYLIST.replace(".ts", ".m4s");
    let segments = load(&playlist, true).await?;

    // Dropping a missing fMP4 segment discards the whole stream, so probes
    // would do more harm than the gap they try to fill.
    assert_eq!(segments.len(), 3);
    assert!(segments.iter().all(|segment| !segment.is_probe));

    Ok(())
}
