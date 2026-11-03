use crate::WriteSegment;
use crate::context::IoriContext;
use crate::util::{Set, Unset};
use crate::{
    IoriError, SegmentInfo, StreamingSegment, StreamingSource, cache::CacheSource,
    download::DownloaderApp, error::IoriResult, merge::Merger,
};
use futures::StreamExt;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::{num::NonZeroU32, sync::Arc};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, Semaphore, oneshot};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
async fn wait_for_stop_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");

    tokio::select! {
        _ = sigint.recv() => {}
        _ = sigterm.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_stop_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

// Forward Ctrl-C immediately to external output workers, including while the
// downloader waits for in-flight downloads or holds the merger lock in finish.
struct OutputStopGuard {
    task: tokio::task::JoinHandle<()>,
    token: Option<tokio_util::sync::CancellationToken>,
}

impl Drop for OutputStopGuard {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(token) = &self.token {
            token.cancel();
        }
    }
}

struct SegmentDownloadOutcome {
    segment: SegmentInfo,
    succeeded: bool,
}

async fn download_segment<S, C>(
    segment: S,
    context: IoriContext,
    cache: Arc<C>,
    retries: u32,
) -> SegmentDownloadOutcome
where
    S: StreamingSegment + WriteSegment + Send + 'static,
    C: CacheSource + Send + Sync + 'static,
{
    let segment_info = SegmentInfo::from(&segment);
    let filename = segment_info.file_name.clone();
    let mut retries = retries;

    loop {
        if retries == 0 {
            return SegmentDownloadOutcome {
                segment: segment_info,
                succeeded: false,
            };
        }

        let writer = cache.open_writer(&segment_info).await.transpose();
        let Some(writer) = writer else {
            return SegmentDownloadOutcome {
                segment: segment_info,
                succeeded: true,
            };
        };

        let mut writer = match writer {
            Ok(writer) => writer,
            Err(e) => {
                tracing::warn!("Failed to open writer for {filename}: {e}. Retrying later.");
                retries -= 1;
                continue;
            }
        };

        // Workaround for `higher-ranked lifetime error`
        let result = segment.write_segment(&context, &mut writer).await;
        let result = match result {
            // graceful shutdown
            Ok(_) => writer.shutdown().await.map_err(IoriError::IOError),
            Err(e) => Err(e),
        };
        drop(writer);

        match result {
            Ok(_) => {
                return SegmentDownloadOutcome {
                    segment: segment_info,
                    succeeded: true,
                };
            }
            Err(e) => {
                // Invalidate the cache on failure.
                _ = cache.invalidate(&segment_info).await;
                tracing::warn!("Processing {filename} failed, retry later. {e}");
                retries -= 1;
            }
        }
    }
}

/// Spawn a task that listens for Ctrl-C signals and stops the downloader
///
/// The first Ctrl-C will trigger a graceful shutdown by calling `stop_signal.stop()`.
/// The second Ctrl-C will force exit the process.
pub fn spawn_ctrlc_handler() -> oneshot::Receiver<()> {
    let (stop_signal, receiver) = oneshot::channel();

    tokio::spawn(async move {
        // wait for the first ctrl-c to stop downloader
        wait_for_stop_signal().await;
        tracing::info!("Ctrl-C received, stopping downloader.");
        let _ = stop_signal.send(());

        // wait for the second ctrl-c to force exit
        wait_for_stop_signal().await;
        tracing::info!("Ctrl-C received again, force exit.");
        std::process::exit(1);
    });

    receiver
}

pub struct ParallelDownloader<S = (), M = (), C = (), A = ()> {
    context: IoriContext,

    source: Arc<S>,
    concurrency: NonZeroU32,
    permits: Arc<Semaphore>,
    dual_cursor: bool,

    app: Arc<A>,

    cache: Arc<C>,
    merger: Arc<Mutex<M>>,

    retries: u32,
    stop_signal: oneshot::Receiver<()>,
}

impl ParallelDownloader {
    pub fn builder(context: IoriContext) -> ParallelDownloaderBuilder {
        ParallelDownloaderBuilder::new(context)
    }
}

impl<S, M, C, A> ParallelDownloader<S, M, C, A>
where
    S: StreamingSource + Send + Sync + 'static,
    S::Segment: StreamingSegment + WriteSegment + Send + 'static,
    M: Merger + Send + Sync + 'static,
    C: CacheSource + Send + Sync + 'static,
    A: DownloaderApp + Send + Sync + 'static,
{
    pub async fn download(mut self) -> IoriResult<M::Result> {
        let token = self.merger.lock().await.cancellation_token();
        let task_token = token.clone();
        let (sender, receiver) = oneshot::channel();
        let stop_signal = std::mem::replace(&mut self.stop_signal, receiver);
        let _stop_guard = OutputStopGuard {
            token,
            task: tokio::spawn(async move {
                if let Some(token) = task_token {
                    tokio::select! {
                        _ = stop_signal => token.cancel(),
                        _ = token.cancelled() => (),
                    }
                } else {
                    let _ = stop_signal.await;
                }
                let _ = sender.send(());
            }),
        };
        self.app.on_start().await?;

        // With `dual_cursor` two cursors share one chronological output: the
        // backfill cursor walks the playlist from its oldest segment under
        // `permits`, while the live-edge cursor fetches everything published
        // after the initial window under its own `edge_permits`. The live-edge
        // cursor therefore keeps the newest segments even when a slow backfill
        // lets the playlist roll them out of the CDN window. Without the flag
        // every batch draws from `permits`, i.e. the original single-cursor
        // order.
        //
        // Downloads run off the dispatch loop so the loop keeps reading the
        // playlist while a batch is still being fetched. Reports return to
        // dispatch order through a one-shot chain, so the merger only ever
        // buffers the segment it is waiting for.
        let cancel = CancellationToken::new();
        let edge_permits = Arc::new(Semaphore::new(self.concurrency.get() as usize));
        let mut tasks = JoinSet::new();
        let mut previous_report: Option<oneshot::Receiver<()>> = None;
        let mut live_edge = false;
        let mut stopped = false;

        {
            let stream = tokio::select! {
                result = self.source.segments_stream(&self.context) => Some(result?),
                _ = &mut self.stop_signal => {
                    tracing::info!("Stop signal received while starting the stream.");
                    None
                }
            };
            if stream.is_none() {
                drop(stream);
                return self.finish().await;
            }
            let stream = stream.expect("stream was checked above");
            tokio::pin!(stream);

            loop {
                while tasks.try_join_next().is_some() {
                    // Reap finished downloads so a live stream cannot accumulate
                    // completed join handles for the whole session.
                }
                let segments = tokio::select! {
                    biased;
                    _ = &mut self.stop_signal => {
                        stopped = true;
                        tracing::info!("Stop signal received, finishing downloaded segments.");
                        break;
                    }
                    segments = stream.next() => segments,
                };

                let Some(segments) = segments else {
                    break;
                };

                // If the playlist is not available, the downloader will be stopped.
                if let Err(e) = segments {
                    tracing::error!("Failed to fetch segment list: {e}");
                    return Err(e);
                }
                let segments = segments?;

                self.app
                    .on_receive_segments(
                        &segments.iter().map(SegmentInfo::from).collect::<Vec<_>>(),
                    )
                    .await;

                let mut synchronized_groups: Vec<Vec<S::Segment>> = Vec::new();
                let mut group_indexes: HashMap<(u64, u64), usize> = HashMap::new();
                for segment in segments {
                    if let Some(key) = segment.synchronization_key() {
                        if let Some(&index) = group_indexes.get(&key) {
                            synchronized_groups[index].push(segment);
                        } else {
                            let index = synchronized_groups.len();
                            group_indexes.insert(key, index);
                            synchronized_groups.push(vec![segment]);
                        }
                    } else {
                        synchronized_groups.push(vec![segment]);
                    }
                }

                for group in synchronized_groups {
                    let synchronized = group.len() > 1
                        && group
                            .iter()
                            .any(|segment| segment.stream_type() == crate::StreamType::Audio)
                        && group
                            .iter()
                            .any(|segment| segment.stream_type() == crate::StreamType::Video);

                    let context = self.context.clone();
                    let app = self.app.clone();
                    let merger = self.merger.clone();
                    let cache = self.cache.clone();
                    let retries = self.retries;
                    let dual_cursor = self.dual_cursor;
                    let edge = dual_cursor && live_edge;
                    // Without --dual-cursor the permit is reserved before
                    // dispatching, so downloads start in playlist order exactly
                    // like the original downloader. With the flag the loop must
                    // never block on a permit: it has to keep reading the
                    // playlist while the backfill drains.
                    let reserved = if dual_cursor {
                        None
                    } else {
                        Some(
                            self.permits
                                .clone()
                                .acquire_owned()
                                .await
                                .expect("download semaphore is never closed"),
                        )
                    };
                    let permits = if edge {
                        edge_permits.clone()
                    } else {
                        self.permits.clone()
                    };
                    let task_cancel = cancel.clone();
                    let (report_tx, report_rx) = oneshot::channel();
                    let previous = previous_report.replace(report_rx);
                    tasks.spawn(async move {
                        let permit = match reserved {
                            Some(permit) => permit,
                            None => {
                                let acquired = tokio::select! {
                                    biased;
                                    _ = task_cancel.cancelled() => None,
                                    permit = permits.acquire_owned() => {
                                        Some(permit.expect("download semaphore is never closed"))
                                    }
                                };
                                match acquired {
                                    Some(permit) => permit,
                                    None => {
                                        // Stopped before this group touched the
                                        // network. Still take the report turn, so a
                                        // later group can never report ahead of an
                                        // earlier one whose download is still
                                        // running.
                                        if let Some(previous) = previous {
                                            let _ = previous.await;
                                        }
                                        let _ = report_tx.send(());
                                        return;
                                    }
                                }
                            }
                        };
                        let mut outcomes = Vec::with_capacity(group.len());
                        for segment in group {
                            outcomes.push(
                                download_segment(segment, context.clone(), cache.clone(), retries)
                                    .await,
                            );
                        }
                        drop(permit);
                        let group_failed = synchronized && outcomes.iter().any(|o| !o.succeeded);

                        if let Some(previous) = previous {
                            let _ = previous.await;
                        }
                        for outcome in outcomes {
                            let filename = outcome.segment.file_name.clone();
                            if group_failed || !outcome.succeeded {
                                app.on_failed_segment(&outcome.segment).await;
                                if let Err(e) = merger
                                    .lock()
                                    .await
                                    .fail(outcome.segment, cache.clone())
                                    .await
                                {
                                    tracing::error!("Failed to mark {filename} as failed: {e}");
                                }
                            } else {
                                app.on_downloaded_segment(&outcome.segment).await;
                                if let Err(e) = merger
                                    .lock()
                                    .await
                                    .update(outcome.segment, cache.clone())
                                    .await
                                {
                                    tracing::error!("Failed to mark {filename} as downloaded: {e}");
                                }
                            }
                        }
                        let _ = report_tx.send(());
                    });
                }
                live_edge = true;
            }
        }

        if stopped {
            // Groups that never started downloading are dropped here; the ones
            // already in flight still finish and report.
            cancel.cancel();
        }
        // A finished playlist (DVR, one-shot batch) leaves the dispatch loop
        // right after queueing everything, so the drain below is the only place
        // that can still observe Ctrl-C. Without this a long backfill cannot be
        // interrupted at all.
        let mut stop_observed = stopped;
        while !tasks.is_empty() {
            tokio::select! {
                biased;
                _ = &mut self.stop_signal, if !stop_observed => {
                    stop_observed = true;
                    tracing::info!("Stop signal received, dropping queued segments.");
                    cancel.cancel();
                }
                _ = tasks.join_next() => {}
            }
        }

        self.finish().await
    }

    async fn finish(self) -> IoriResult<M::Result> {
        self.app.on_finished().await?;
        self.merger.lock().await.finish(self.cache).await
    }
}

pub struct ParallelDownloaderBuilder<
    M = (),
    C = (),
    MR = (),
    A = (),
    MergerSet = Unset,
    CacheSet = Unset,
    AppSet = Unset,
> {
    context: IoriContext,

    concurrency: NonZeroU32,
    retries: u32,
    dual_cursor: bool,
    merger: Option<M>,
    cache: Option<C>,
    stop_signal: Option<oneshot::Receiver<()>>,
    app: Option<A>,

    _merge_result: PhantomData<MR>,
    _set: (
        PhantomData<MergerSet>,
        PhantomData<CacheSet>,
        PhantomData<AppSet>,
    ),
}

impl<M, C, MR, A, MergerSet, CacheSet, AppSet>
    ParallelDownloaderBuilder<M, C, MR, A, MergerSet, CacheSet, AppSet>
{
    fn new(context: IoriContext) -> Self {
        Self {
            context,
            concurrency: NonZeroU32::new(5).unwrap(),
            retries: 3,
            dual_cursor: false,
            merger: None,
            cache: None,
            stop_signal: None,
            app: None,
            _merge_result: Default::default(),
            _set: Default::default(),
        }
    }

    pub fn concurrency(mut self, concurrency: NonZeroU32) -> Self {
        self.concurrency = concurrency;
        self
    }

    pub fn retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// Enables the live-edge cursor: segments published after the initial
    /// playlist window are fetched immediately instead of queueing behind the
    /// backfill. Off by default.
    pub fn dual_cursor(mut self, dual_cursor: bool) -> Self {
        self.dual_cursor = dual_cursor;
        self
    }

    pub fn stop_signal(mut self, stop_signal: oneshot::Receiver<()>) -> Self {
        self.stop_signal = Some(stop_signal);
        self
    }

    pub fn ctrlc_handler(mut self) -> Self {
        self.stop_signal = Some(spawn_ctrlc_handler());
        self
    }

    pub fn merger<MM>(
        self,
        merger: MM,
    ) -> ParallelDownloaderBuilder<MM, C, MR, A, Set, CacheSet, AppSet>
    where
        MM: Merger<Result = MR> + Send + Sync + 'static,
    {
        ParallelDownloaderBuilder {
            context: self.context,
            concurrency: self.concurrency,
            retries: self.retries,
            dual_cursor: self.dual_cursor,
            merger: Some(merger),
            cache: self.cache,
            stop_signal: self.stop_signal,
            app: self.app,
            _merge_result: Default::default(),
            _set: Default::default(),
        }
    }

    pub fn cache<CC>(
        self,
        cache: CC,
    ) -> ParallelDownloaderBuilder<M, CC, MR, A, MergerSet, Set, AppSet>
    where
        CC: CacheSource,
    {
        ParallelDownloaderBuilder {
            context: self.context,
            concurrency: self.concurrency,
            retries: self.retries,
            dual_cursor: self.dual_cursor,
            merger: self.merger,
            cache: Some(cache),
            stop_signal: self.stop_signal,
            app: self.app,
            _merge_result: Default::default(),
            _set: Default::default(),
        }
    }

    pub fn app<AA>(
        self,
        app: AA,
    ) -> ParallelDownloaderBuilder<M, C, MR, AA, MergerSet, CacheSet, Set>
    where
        AA: DownloaderApp + Send + Sync + 'static,
    {
        ParallelDownloaderBuilder {
            context: self.context,
            concurrency: self.concurrency,
            retries: self.retries,
            dual_cursor: self.dual_cursor,
            merger: self.merger,
            cache: self.cache,
            stop_signal: self.stop_signal,
            app: Some(app),
            _merge_result: Default::default(),
            _set: Default::default(),
        }
    }
}

impl<M, C, MR, A, MS, CS, AS> Default for ParallelDownloaderBuilder<M, C, MR, A, MS, CS, AS> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<M, C, MR, A> ParallelDownloaderBuilder<M, C, MR, A, Set, Set, Set>
where
    M: Merger<Result = MR> + Send + Sync + 'static,
    C: CacheSource,
    A: DownloaderApp + Send + Sync + 'static,
{
    fn build<S>(self, source: S) -> ParallelDownloader<S, M, C, A>
    where
        S: StreamingSource + Send + Sync + 'static,
    {
        ParallelDownloader {
            context: self.context,
            app: Arc::new(self.app.expect("App is not set")),
            source: Arc::new(source),
            merger: Arc::new(Mutex::new(self.merger.expect("Merger is not set"))),
            cache: Arc::new(self.cache.expect("Cache is not set")),
            concurrency: self.concurrency,
            permits: Arc::new(Semaphore::new(self.concurrency.get() as usize)),
            retries: self.retries,
            dual_cursor: self.dual_cursor,
            stop_signal: self.stop_signal.expect("Stop signal is not set"),
        }
    }

    pub async fn download<S>(self, source: S) -> IoriResult<MR>
    where
        S: StreamingSource + Send + Sync + 'static,
    {
        let downloader = self.build(source);
        downloader.download().await
    }
}
