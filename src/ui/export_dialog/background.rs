//! The Results panel keeps long export jobs reachable after their form closes.
use super::*;
use gpui_kit::component::scroll::ScrollableElement;
use std::io::{self, Write};

pub(in crate::ui) struct Entry {
    view: Entity<ExportDialog>,
    _subscription: Subscription,
}

type Work = Box<dyn FnOnce() + Send>;
pub(super) type WriterSender = std::sync::mpsc::SyncSender<Work>;

/// Start the writer before admitting a download. Dropping this sender stops
/// the idle thread when source admission fails or the form closes.
pub(super) fn writer() -> io::Result<WriterSender> {
    let (sender, work) = std::sync::mpsc::sync_channel::<Work>(1);
    std::thread::Builder::new()
        .name("qrow-export".into())
        .spawn(move || {
            if let Ok(work) = work.recv() {
                work();
            }
        })?;
    Ok(sender)
}

pub(super) struct Output<'a, W> {
    pub out: &'a mut W,
    pub bytes: &'a AtomicU64,
}

impl<W: Write> Write for Output<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.out.write(bytes)?;
        self.bytes.fetch_add(written as u64, Ordering::Relaxed);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

impl Qrow {
    pub(in crate::ui) fn export_spool(
        &self,
        tab: Uuid,
        execution: Option<ExecutionId>,
        exclude: Option<EntityId>,
        cx: &App,
    ) -> Option<Arc<export::spool::Spool>> {
        self.exports.iter().find_map(|entry| {
            if exclude == Some(entry.view.entity_id()) {
                return None;
            }
            let job = entry.view.read(cx);
            (job.result.tab == tab && job.result.execution == execution)
                .then(|| job.retained.as_ref().or(job.result.replay.as_ref()))
                .flatten()
                .filter(|spool| matches!(spool.status(), export::spool::Status::Complete { .. }))
                .cloned()
        })
    }

    pub(in crate::ui) fn export_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let jobs = v_flex()
            .w_full()
            .h(rems((self.exports.len() as f32 * 2.25).min(8.)))
            .min_h_0()
            .flex_shrink_0()
            .children(self.exports.iter().map(|entry| {
                let job = entry.view.read(cx);
                let id = job.id;
                let message = job.status_text();
                let details = entry.view.clone();
                let cancel = entry.view.downgrade();
                h_flex()
                    .id(SharedString::from(format!("export-job-{id}")))
                    .test_support()
                    .min_w_0()
                    .h_9()
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .id("status")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(message.clone())
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_ellipsis()
                            .text_color(if job.error.is_some() {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(message),
                    )
                    .child(
                        Button::new(SharedString::from(format!("export-details-{id}")))
                            .small()
                            .ghost()
                            .label(
                                if !job.running
                                    && job.retained.as_ref().is_some_and(|spool| {
                                        matches!(
                                            spool.status(),
                                            export::spool::Status::Complete { .. }
                                        )
                                    })
                                {
                                    "Retry…"
                                } else {
                                    "Details…"
                                },
                            )
                            .on_click(move |_, window, cx| {
                                ExportDialog::open(&details, window, cx)
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("export-stop-{id}")))
                            .small()
                            .ghost()
                            .label(if job.running {
                                "Cancel export"
                            } else {
                                "Dismiss"
                            })
                            .on_click(move |_, _, cx| {
                                let _ = cancel.update(cx, |job, cx| {
                                    if job.running {
                                        job.cancel.store(true, Ordering::Relaxed);
                                        job.cancel_download();
                                        cx.notify();
                                    } else {
                                        job.remove_background(cx);
                                    }
                                });
                            }),
                    )
            }))
            .overflow_y_scrollbar()
            .id("export-jobs-scroll");
        div()
            .id("export-jobs")
            .test_support()
            .w_full()
            .flex_shrink_0()
            .child(jobs)
    }
}

impl ExportDialog {
    pub(super) fn start_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.background = true;
        let view = cx.entity();
        let _ = self.owner.update(cx, |owner, cx| {
            if !owner.exports.iter().any(|entry| entry.view == view) {
                let subscription = cx.observe(&view, |_, _, cx| cx.notify());
                owner.exports.push(Entry {
                    view: view.clone(),
                    _subscription: subscription,
                });
            }
            cx.notify();
        });
        if let Some(handle) = self.dialog.take() {
            Root::update(window, cx, |root, window, cx| {
                root.close_dialog_for(&handle, window, cx)
            });
        }
        self.progress_task = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if !weak
                    .update(cx, |job, cx| {
                        if let Some(spool) =
                            job.download.as_ref().and_then(|download| download.spool())
                        {
                            job.retained = Some(spool);
                        }
                        job.refresh_preview_source(cx);
                        cx.notify();
                        job.running
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        }));
    }

    pub(super) fn remove_background(&self, cx: &mut Context<Self>) {
        let view = cx.entity();
        let _ = self.owner.update(cx, |owner, cx| {
            owner.exports.retain(|entry| entry.view != view);
            cx.notify();
        });
    }

    pub(super) fn status_text(&self) -> String {
        let filename = self
            .output_path
            .as_deref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.filename.clone());
        if let Some(error) = &self.error {
            return format!("{filename}: {error}");
        }
        if !self.running {
            return format!("{filename}: Ready to retry");
        }
        if self.cancel.load(Ordering::Relaxed) {
            return format!("{filename}: Stopping export…");
        }
        let elapsed = self
            .started
            .map_or(0., |start| start.elapsed().as_secs_f64());
        let query = self
            .download
            .as_ref()
            .and_then(|download| download.progress_percentage())
            .map_or(String::new(), |value| format!(", query {value:.0}%"));
        if self.download.is_some() && self.retained.is_none() {
            return format!("{filename}: Preparing export, {elapsed:.0}s{query}");
        }
        if let Some(spool) = &self.retained
            && matches!(spool.status(), export::spool::Status::Downloading)
        {
            let rows = spool.row_count();
            let mib = spool.bytes() as f64 / (1024. * 1024.);
            return format!(
                "{filename}: Downloading {rows} rows, {mib:.1} MiB, {:.0} rows/s, {elapsed:.0}s{query}",
                rows as f64 / elapsed.max(0.001)
            );
        }
        let rows = self.retained.as_ref().map_or(
            self.source.table(self.range()).row_count() as u64,
            |spool| spool.row_count(),
        );
        let mib = self.output_bytes.load(Ordering::Relaxed) as f64 / (1024. * 1024.);
        format!(
            "{filename}: Writing {rows} rows, {mib:.1} MiB, {:.1} MiB/s, {elapsed:.0}s",
            mib / elapsed.max(0.001)
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::connector::{Cancellation, Connector, MetadataRequest, QueryState, Secret, Session};
    use crate::model::Profile;
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct ProgressSession(Arc<AtomicBool>);
    impl Cancellation for ProgressSession {
        fn cancel(&self) -> anyhow::Result<()> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    impl Connector for ProgressSession {
        fn connect(&self, _: &Profile, _: Secret) -> anyhow::Result<Box<dyn Session>> {
            Ok(Box::new(Self(self.0.clone())))
        }
    }
    impl Session for ProgressSession {
        fn execute(&mut self, _: &str) -> anyhow::Result<Arc<dyn Cancellation>> {
            Ok(Arc::new(Self(self.0.clone())))
        }
        fn execute_metadata(
            &mut self,
            _: &MetadataRequest,
        ) -> anyhow::Result<Arc<dyn Cancellation>> {
            unreachable!()
        }
        fn poll(&mut self) -> anyhow::Result<QueryState> {
            Ok(if self.0.load(Ordering::SeqCst) {
                QueryState::Cancelled
            } else {
                QueryState::Running
            })
        }
        fn progress_percentage(&self) -> Option<f64> {
            Some(42.)
        }
        fn columns(&mut self) -> anyhow::Result<Vec<crate::model::Column>> {
            unreachable!()
        }
        fn fetch(&mut self, _: usize) -> anyhow::Result<crate::model::Batch> {
            unreachable!()
        }
        fn close_operation(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn execute_keep_alive(&mut self, _: &str) -> anyhow::Result<Arc<dyn Cancellation>> {
            unreachable!()
        }
        fn close_keep_alive(&mut self) -> anyhow::Result<()> {
            unreachable!()
        }
        fn close(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[gpui_kit::test]
    fn details_exposes_wrapping_progress_before_and_after_the_schema(cx: &mut TestAppContext) {
        use super::*;
        let worker = crate::worker::Worker::with_connector(
            Arc::new(|| {}),
            Arc::new(ProgressSession(Arc::new(AtomicBool::new(false)))),
            Arc::new(|_| Ok(Secret::password(""))),
        );
        let flag = Arc::new(AtomicBool::new(false));
        let download = worker
            .run_and_export(
                Profile {
                    host: "synthetic.invalid".into(),
                    username: "synthetic".into(),
                    ..Default::default()
                },
                "SELECT 1".into(),
                ExecutionId(42),
                None,
                &export::Jobs::default(),
                flag.clone(),
            )
            .unwrap();
        loop {
            if matches!(
                worker.events.recv_timeout(Duration::from_secs(3)).unwrap(),
                Event::DownloadProgress {
                    percentage: Some(42.),
                    ..
                }
            ) {
                break;
            }
        }
        cx.update(crate::ui::init);
        let mut owner = None;
        let window = cx.open_window(size(px(900.), px(760.)), |window, cx| {
            let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
            owner = Some(qrow.downgrade());
            crate::ui::root(qrow, window, cx)
        });
        let owner = owner.unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            let tab = owner.upgrade().unwrap().read(cx).tabs[0].saved.id;
            let columns = [crate::model::Column { name: "n".into(), data_type: "INT".into() }];
            let source = Arc::new(Snapshot::new(&columns, &export::Rows::default()).unwrap());
            let view = cx.new(|cx| ExportDialog::new(source, None, export::Settings::default(),
                "a-long-export-filename-with-many-words-to-force-a-second-line-of-progress-metrics.csv".into(), false,
                ExportResult { tab, execution: Some(ExecutionId(42)), cursor: crate::worker::Cursor::Draining, replay: None },
                owner, window, cx));
            view.update(cx, |job, _| {
                job.running = true;
                job.download = Some(download.clone());
                job.started = Some(Instant::now() - Duration::from_secs(10));
            });
            ExportDialog::open(&view, window, cx);
            window.render_frame(cx);
            let preparing = window.find("export-progress");
            assert!(preparing.visible());
            assert!(preparing.label().unwrap().contains("Preparing export, 10s, query 42%"));
            assert!(preparing.bounds().size.height > window.rem_size() * 1.5);
            let (spool, mut producer) = export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
            producer.append(&[vec![Some("1".into())]], &flag).unwrap();
            view.update(cx, |job, cx| { job.retained = Some(spool); cx.notify(); });
            window.render_frame(cx);
            let downloading = window.find("export-progress");
            assert!(downloading.label().unwrap().contains("Downloading 1 rows"));
            assert!(downloading.label().unwrap().contains("MiB,"));
            assert!(downloading.label().unwrap().contains("rows/s, 10s, query 42%"));
            assert!(window.find("export-cancel").visible());
            window.click("export-save", cx);
            assert!(view.read(cx).running);
            assert!(!flag.load(Ordering::Relaxed));
            assert_eq!(view.read(cx).output_path, None);
        }).unwrap();
        download.cancel();
        worker.shutdown();
        worker.wait_for_shutdown(Duration::from_secs(3));
    }

    #[gpui_kit::test]
    fn retained_jobs_scroll_to_their_retry_and_dismiss_controls(cx: &mut TestAppContext) {
        use super::*;
        use gpui_kit::InputEvent as _;
        cx.update(crate::ui::init);
        let mut owner = None;
        let window = cx.open_window(size(px(900.), px(620.)), |window, cx| {
            let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
            owner = Some(qrow.downgrade());
            crate::ui::root(qrow, window, cx)
        });
        let owner = owner.unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            let tab = owner.upgrade().unwrap().read(cx).tabs[0].saved.id;
            let columns = [crate::model::Column {
                name: "n".into(),
                data_type: "INT".into(),
            }];
            let rows = export::Rows::from(vec![vec![Some("1".into())]]);
            let source = Arc::new(Snapshot::new(&columns, &rows).unwrap());
            let (spool, producer) =
                export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
            producer.finish(&AtomicBool::new(false)).unwrap();
            let mut last = None;
            for index in 0..8 {
                let result = ExportResult {
                    tab,
                    execution: None,
                    cursor: crate::worker::Cursor::Downloaded,
                    replay: Some(spool.clone()),
                };
                let view = cx.new(|cx| {
                    ExportDialog::new(
                        source.clone(),
                        None,
                        export::Settings::default(),
                        format!("job-{index}"),
                        false,
                        result,
                        owner.clone(),
                        window,
                        cx,
                    )
                });
                view.update(cx, |job, cx| {
                    job.running = true;
                    job.retained = Some(spool.clone());
                    job.start_background(window, cx);
                    job.complete(
                        Err(io::Error::other("Output volume is full")),
                        None,
                        None,
                        window,
                        cx,
                    );
                });
                last = Some(view.read(cx).id);
            }
            let last = last.unwrap();
            let details = SharedString::from(format!("export-details-{last}"));
            let dismiss = SharedString::from(format!("export-stop-{last}"));
            window.render_frame(cx);
            let list = window.find("export-jobs");
            assert!(list.bounds().size.height > px(0.));
            assert!(list.bounds().size.height <= window.rem_size() * 8.);
            assert!(!window.find(details.clone()).visible());
            for _ in 0..8 {
                let position = window.find("export-jobs").bounds().center();
                window.dispatch_event(
                    ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
                window.render_frame(cx);
            }
            assert!(window.find(details).visible());
            assert!(window.find(dismiss.clone()).visible());
            assert!(
                window
                    .find("export-jobs")
                    .bounds()
                    .contains(&window.find(dismiss.clone()).bounds().center())
            );
            window.click(dismiss, cx);
            assert_eq!(owner.upgrade().unwrap().read(cx).exports.len(), 7);
        })
        .unwrap();
    }
}
