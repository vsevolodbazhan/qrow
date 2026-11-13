use super::{CopyRequest, copy_text};
use gpui_kit::{ClipboardItem, Image, ImageFormat, TestAppContext};

/// Notifications slide in on the real clock, and a click renders between
/// the press and the release. Wait until `id` stops moving, so that the
/// press and the release land on it.
pub(super) fn wait_until_still(
    window: &mut gpui_kit::Window,
    id: gpui_kit::SharedString,
    cx: &mut gpui_kit::App,
) {
    use gpui_kit::test::TestWindowExt;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last = None;
    loop {
        window.render_frame(cx);
        let bounds = window.find(id.clone()).bounds();
        if last == Some(bounds) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{id} kept moving");
        last = Some(bounds);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[gpui_kit::test]
fn replay_publication_holds_the_profile_slot_and_retry_keeps_it_after_tab_close(
    cx: &mut TestAppContext,
) {
    use super::*;
    cx.update(crate::ui::init);
    let mut owner = None;
    let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
        let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
        owner = Some(qrow.downgrade());
        crate::ui::root(qrow, window, cx)
    });
    let owner = owner.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let failed = directory.path().join("directory.csv");
    std::fs::create_dir(&failed).unwrap();
    let output = directory.path().join("retry.csv");
    let (sender, work) = std::sync::mpsc::sync_channel(1);
    let dialog = cx
        .update_window(window.into(), |_, window, cx| {
            let (source, mut result) = owner
                .update(cx, |owner, cx| {
                    let tab = &owner.tabs[0];
                    let data = tab.shown_result().table.read(cx).delegate();
                    (
                        Arc::new(Snapshot::new(&data.columns, &data.rows).unwrap()),
                        ExportResult {
                            tab: tab.saved.id,
                            execution: tab.current_execution,
                            cursor: crate::worker::Cursor::Complete,
                            replay: None,
                        },
                    )
                })
                .unwrap();
            let columns = [crate::model::Column {
                name: "n".into(),
                data_type: "INT".into(),
            }];
            let (spool, mut producer) =
                export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
            producer
                .append(&[vec![Some("42".into())]], &AtomicBool::new(false))
                .unwrap();
            producer.finish(&AtomicBool::new(false)).unwrap();
            result.replay = Some(spool);
            let profile = owner.upgrade().unwrap().read(cx).profiles[0].clone();
            let dialog = cx.new(|cx| {
                ExportDialog::new(
                    source,
                    export::Settings::default(),
                    "publication".into(),
                    false,
                    result,
                    Some(profile),
                    owner.clone(),
                    window,
                    cx,
                )
            });
            ExportDialog::open(&dialog, window, cx);
            dialog.update(cx, |dialog, cx| {
                dialog.scope = Scope::Replay;
                dialog.start_with_writer(true, move || Ok(sender), window, cx);
            });
            dialog
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| Some(failed.clone()));
    cx.run_until_parked();
    let first_work = work.recv_timeout(Duration::from_secs(3)).unwrap();
    let (jobs, profile, old_cancel) = cx
        .update_window(window.into(), |_, window, cx| {
            let job = dialog.read(cx);
            let profile = job.profile.as_ref().unwrap().clone();
            let jobs = cx.global::<export::Jobs>().clone();
            assert_eq!(jobs.active_count(), 1);
            assert!(
                jobs.register_for_profile(Arc::new(AtomicBool::new(false)), None, profile.id, 1)
                    .is_err()
            );
            let old_cancel = job.cancel.clone();
            let old_tab = job.result.tab;
            owner
                .update(cx, |owner, cx| owner.close_tab(0, window, cx))
                .unwrap();
            assert!(
                owner
                    .upgrade()
                    .unwrap()
                    .read(cx)
                    .tabs
                    .iter()
                    .all(|tab| tab.saved.id != old_tab)
            );
            (jobs, profile, old_cancel)
        })
        .unwrap();
    assert!(failed.is_dir());
    first_work();
    // Publication and cleanup release the permit before the UI handles completion.
    assert_eq!(jobs.active_count(), 0);
    assert!(failed.is_dir());
    cx.run_until_parked();
    let (sender, work) = std::sync::mpsc::sync_channel(1);
    cx.update_window(window.into(), |_, window, cx| {
        assert!(dialog.read(cx).error.is_some());
        old_cancel.store(true, Ordering::Relaxed);
        dialog.update(cx, |dialog, cx| {
            dialog.start_with_writer(true, move || Ok(sender), window, cx)
        });
    })
    .unwrap();
    cx.simulate_new_path_selection(|_| Some(output.clone()));
    cx.run_until_parked();
    let retry_work = work.recv_timeout(Duration::from_secs(3)).unwrap();
    cx.update_window(window.into(), |_, _, cx| {
        let job = dialog.read(cx);
        assert_eq!(job.profile.as_ref().unwrap().id, profile.id);
        assert!(!Arc::ptr_eq(&old_cancel, &job.cancel));
        assert!(old_cancel.load(Ordering::Relaxed));
        assert!(!job.cancel.load(Ordering::Relaxed));
    })
    .unwrap();
    assert_eq!(jobs.active_count(), 1);
    retry_work();
    assert_eq!(jobs.active_count(), 0);
    assert_eq!(std::fs::read_to_string(output).unwrap(), "n\r\n42\r\n");
    cx.run_until_parked();
}

#[gpui_kit::test]
fn writer_thread_failure_leaves_the_cursor_available_and_reports_the_error(
    cx: &mut TestAppContext,
) {
    use super::*;
    cx.update(crate::ui::init);
    let mut owner = None;
    let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
        let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
        owner = Some(qrow.downgrade());
        crate::ui::root(qrow, window, cx)
    });
    let owner = owner.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("result.csv");
    let dialog = cx
        .update_window(window.into(), |_, window, cx| {
            let (source, result) = owner
                .update(cx, |owner, cx| {
                    let tab = &mut owner.tabs[0];
                    tab.cursor = crate::worker::Cursor::Available;
                    let data = tab.shown_result().table.read(cx).delegate();
                    (
                        Arc::new(Snapshot::new(&data.columns, &data.rows).unwrap()),
                        ExportResult {
                            tab: tab.saved.id,
                            execution: None,
                            cursor: crate::worker::Cursor::Available,
                            replay: None,
                        },
                    )
                })
                .unwrap();
            let dialog = cx.new(|cx| {
                ExportDialog::new(
                    source,
                    export::Settings::default(),
                    "thread-failure".into(),
                    true,
                    result,
                    None,
                    owner.clone(),
                    window,
                    cx,
                )
            });
            ExportDialog::open(&dialog, window, cx);
            dialog.update(cx, |dialog, cx| {
                dialog.scope = Scope::All;
                dialog.start_with_writer(
                    true,
                    || Err(std::io::Error::other("No thread resources")),
                    window,
                    cx,
                );
            });
            dialog
        })
        .unwrap();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, _, cx| {
        let dialog = dialog.read(cx);
        assert!(!dialog.running);
        assert_eq!(dialog.error.as_deref(), Some("No thread resources"));
        assert!(dialog.download.is_none());
        assert!(dialog.retained.is_none());
        let owner = owner.upgrade().unwrap();
        let owner = owner.read(cx);
        assert!(!owner.tabs[0].busy);
        assert_eq!(owner.tabs[0].cursor, crate::worker::Cursor::Available);
        assert!(owner.tabs[0].download.is_none());
        assert!(owner.exports.is_empty());
        assert_eq!(cx.global::<export::Jobs>().active_count(), 0);
    })
    .unwrap();
    assert!(!path.exists());
}

#[gpui_kit::test]
fn background_export_survives_tab_close_and_keeps_failure_until_dismissed(cx: &mut TestAppContext) {
    use super::*;
    use gpui_kit::test::TestWindowExt;
    cx.update(crate::ui::init);
    let mut owner = None;
    let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
        let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
        owner = Some(qrow.downgrade());
        crate::ui::root(qrow, window, cx)
    });
    let owner = owner.unwrap();
    let mut retained = None;
    let mut notice_id = None;
    cx.update_window(window.into(), |_, window, cx| {
            let (source, mut result) = owner
                .update(cx, |owner, cx| {
                    let tab = &owner.tabs[0];
                    let data = tab.shown_result().table.read(cx).delegate();
                    (
                        Arc::new(Snapshot::new(&data.columns, &data.rows).unwrap()),
                        ExportResult {
                            tab: tab.saved.id,
                            execution: None,
                            cursor: crate::worker::Cursor::Complete,
                            replay: None,
                        },
                    )
                })
                .unwrap();
            let old_tab = result.tab;
            let columns = [crate::model::Column {
                name: "n".into(),
                data_type: "INT".into(),
            }];
            let (spool, producer) =
                export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
            let mut producer = producer;
            producer
                .append(
                    &[
                        vec![Some("1".into())],
                        vec![Some("2".into())],
                        vec![Some("3".into())],
                    ],
                    &AtomicBool::new(false),
                )
                .unwrap();
            producer.finish(&AtomicBool::new(false)).unwrap();
            retained = Some(Arc::downgrade(&spool));
            result.replay = Some(spool.clone());
            let dialog = cx.new(|cx| {
                ExportDialog::new(
                    source,
                    export::Settings::default(),
                    "background".into(),
                    false,
                    result,
                    None,
                    owner.clone(),
                    window,
                    cx,
                )
            });
            ExportDialog::open(&dialog, window, cx);
            dialog.update(cx, |dialog, cx| {
                dialog.running = true;
                dialog.output_path = Some(PathBuf::from("a-very-long-export-filename-that-needs-to-be-truncated-in-the-progress-notification.csv"));
                dialog.scope = Scope::Replay;
                dialog.result.replay = Some(spool);
                let jobs = cx.global::<export::Jobs>().clone();
                assert_eq!(
                    dialog
                        .all_source(&jobs, cx)
                        .unwrap()
                        .unwrap()
                        .spool()
                        .unwrap()
                        .row_count(),
                    3
                );
                dialog.start_background(window, cx);
            });
            assert!(Root::read(window, cx).dialog_focus_handle().is_none());
            assert!(!dialog.read(cx).cancel.load(Ordering::Relaxed));
            let id = dialog.read(cx).id;
            window.render_frame(cx);
            let notice = window.within(SharedString::from(format!("export-job-{id}")));
            let title = notice.find("title");
            assert_eq!(title.label(), Some("Exporting results"));
            let heading = notice.find("export-notification-filename");
            assert!(heading.bounds().top() >= title.bounds().bottom());
            assert_eq!(heading.label(), Some("a-very-long-export-filename-that-needs-to-be-truncated-in-the-progress-notification.csv"));
            let status = notice.find("status");
            let detail = status.label().unwrap();
            let cancel_button = notice.find(SharedString::from(format!("export-stop-{id}")));
            // The only action shares the row with the text.
            assert!(cancel_button.bounds().left() >= status.bounds().right());
            assert!(cancel_button.bounds().top() < status.bounds().bottom());
            assert!(cancel_button.bounds().bottom() > title.bounds().top());
            assert!(detail.starts_with("Writing 3 rows"));
            assert!(!detail.contains(".csv"));
            assert!(
                window
                    .within(SharedString::from(format!("export-job-{id}")))
                    .find("status")
                    .label()
                    .unwrap()
                    .contains("Writing 3 rows")
            );
            owner
                .update(cx, |owner, cx| {
                    assert_eq!(owner.exports.len(), 1);
                    owner.close_tab(0, window, cx);
                    assert!(owner.tabs.iter().all(|tab| tab.saved.id != old_tab));
                    assert_eq!(owner.exports.len(), 1);
                })
                .unwrap();
            ExportDialog::open(&dialog, window, cx);
            window.close_dialog(cx);
            assert!(!dialog.read(cx).cancel.load(Ordering::Relaxed));
            dialog.update(cx, |dialog, cx| {
                dialog.complete(
                    Err(std::io::Error::other("Output volume is full")),
                    None,
                    None,
                    window,
                    cx,
                )
            });
            let id = dialog.read(cx).id;
            window.render_frame(cx);
            assert_eq!(
                window
                    .find(SharedString::from(format!("export-details-{id}")))
                    .label(),
                Some("Retry…")
            );
            assert_eq!(
                dialog.read(cx).error.as_deref(),
                Some("Output volume is full")
            );
            dialog.update(cx, |dialog, cx| {
                dialog.settings.format = Format::JsonLines;
                dialog.sync(window, cx);
            });
            window.render_frame(cx);
            assert!(
                window
                    .within(SharedString::from(format!("export-job-{id}")))
                    .find("status")
                    .label()
                    .unwrap()
                    .contains("Ready to retry")
            );
            // The format change cleared the error, but the job is still
            // retained and must stay retryable.
            assert_eq!(
                window
                    .find(SharedString::from(format!("export-details-{id}")))
                    .label(),
                Some("Retry…")
            );
            assert!(retained.as_ref().unwrap().upgrade().is_some());
            notice_id = Some(id);
        })
        .unwrap();
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let id = notice_id.unwrap();
        let stop = SharedString::from(format!("export-stop-{id}"));
        wait_until_still(window, stop.clone(), cx);
        window.click(stop, cx);
        assert!(owner.upgrade().unwrap().read(cx).exports.is_empty());
    })
    .unwrap();
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    assert!(retained.unwrap().upgrade().is_none());
}

#[gpui_kit::test]
fn writer_failure_keeps_retry_and_disconnect_preserves_a_complete_preview(cx: &mut TestAppContext) {
    use super::*;
    use gpui_kit::test::TestWindowExt;
    cx.update(crate::ui::init);
    let mut owner = None;
    let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
        let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
        owner = Some(qrow.downgrade());
        crate::ui::root(qrow, window, cx)
    });
    let owner = owner.unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        let (source, result, incomplete) = owner
            .update(cx, |owner, cx| {
                Qrow::apply_worker_event(
                    &mut owner.tabs[0],
                    Event::Disconnected,
                    true,
                    &[],
                    2048 * 1024 * 1024,
                    cx,
                );
                let tab = &owner.tabs[0];
                assert!(tab.preview_complete);
                let data = tab.shown_result().table.read(cx).delegate();
                (
                    Arc::new(Snapshot::new(&data.columns, &data.rows).unwrap()),
                    ExportResult {
                        tab: tab.saved.id,
                        execution: tab.current_execution,
                        cursor: tab.cursor,
                        replay: None,
                    },
                    !tab.preview_complete,
                )
            })
            .unwrap();
        let dialog = cx.new(|cx| {
            ExportDialog::new(
                source,
                export::Settings::default(),
                "test".into(),
                incomplete,
                result,
                None,
                owner.clone(),
                window,
                cx,
            )
        });
        let jobs = cx.global::<export::Jobs>().clone();
        dialog.update(cx, |dialog, cx| {
            dialog.scope = Scope::All;
            assert!(dialog.all_source(&jobs, cx).unwrap().is_none());
        });
        let columns = [crate::model::Column {
            name: "n".into(),
            data_type: "INT".into(),
        }];
        let (spool, producer) =
            export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
        producer.finish(&AtomicBool::new(false)).unwrap();
        ExportDialog::open(&dialog, window, cx);
        dialog.update(cx, |dialog, cx| {
            dialog.retained = Some(spool);
            // Failure propagation stops the download with this flag. It is
            // distinct from the user dismissing a cancelled operation.
            dialog.cancel.store(true, Ordering::Relaxed);
            dialog.complete(
                Err(std::io::Error::other("Output volume is full")),
                None,
                None,
                window,
                cx,
            );
            assert_eq!(dialog.error.as_deref(), Some("Output volume is full"));
            assert!(!dialog.running);
        });
        window.render_frame(cx);
        assert_eq!(
            window.try_find("export-save").unwrap().label(),
            Some("Save")
        );
        let (failed, _producer) =
            export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
        failed.fail("Spool volume is full");
        dialog.update(cx, |dialog, cx| {
            dialog.retained = Some(failed);
            dialog.complete(
                Err(std::io::Error::other(export::Cancelled)),
                None,
                None,
                window,
                cx,
            );
            assert_eq!(dialog.error.as_deref(), Some("Spool volume is full"));
        });
        window.close_dialog(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn completed_export_preserves_an_unanswered_quit_warning(cx: &mut TestAppContext) {
    use super::*;
    use gpui_kit::test::TestWindowExt;
    use std::sync::mpsc;

    cx.update(crate::ui::init);
    let mut owner = None;
    let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
        let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
        owner = Some(qrow.downgrade());
        crate::ui::root(qrow, window, cx)
    });
    let owner = owner.unwrap();
    let source = Arc::new(
        Snapshot::new(
            &[crate::model::Column {
                name: "value".into(),
                data_type: "STRING".into(),
            }],
            &vec![vec![Some("complete".into())]].into(),
        )
        .unwrap(),
    );
    let (dialog, cancel, jobs, original_focus) = cx
        .update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let original_focus = window.focused(cx);
            let dialog = cx.new(|cx| {
                ExportDialog::new(
                    source.clone(),
                    export::Settings::default(),
                    "test".into(),
                    false,
                    ExportResult {
                        tab: Uuid::nil(),
                        execution: None,
                        cursor: crate::worker::Cursor::Complete,
                        replay: None,
                    },
                    None,
                    owner.clone(),
                    window,
                    cx,
                )
            });
            ExportDialog::open(&dialog, window, cx);
            dialog.update(cx, |dialog, _| dialog.running = true);
            (
                dialog.clone(),
                dialog.read(cx).cancel.clone(),
                cx.global::<export::Jobs>().clone(),
                original_focus,
            )
        })
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("result.csv");
    let writer_path = path.clone();
    let guard = jobs.register(cancel.clone());
    let (started, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _guard = guard;
        export::save(&writer_path, &cancel, |out| {
            let count =
                export::write_csv(out, &source.table(None), &CsvOptions::default(), &cancel)?;
            started.send(()).unwrap();
            released.recv().unwrap();
            Ok(count)
        })
        .map(|count| (count, None))
    });
    ready.recv().unwrap();
    assert_eq!(jobs.active_count(), 1);
    cx.update_window(window.into(), |_, window, cx| {
        owner
            .update(cx, |qrow, cx| qrow.request_quit(window, cx))
            .unwrap();
        window.render_frame(cx);
        assert!(window.try_find("keep-working").is_some());
    })
    .unwrap();
    release.send(()).unwrap();
    let result = worker.join().unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "value\r\ncomplete\r\n"
    );
    assert_eq!(jobs.active_count(), 0);
    cx.update_window(window.into(), |_, window, cx| {
        let warning = Root::read(window, cx)
            .dialog_focus_handle()
            .cloned()
            .unwrap();
        dialog.update(cx, |dialog, cx| {
            dialog.complete(result, Some(path), None, window, cx)
        });
        window.render_frame(cx);
        assert_eq!(Root::read(window, cx).dialog_focus_handle(), Some(&warning));
        assert!(window.try_find("keep-working").is_some());
        assert!(window.try_find("export-save").is_none());
        assert!(owner.upgrade().unwrap().read(cx).quit_warning_open);
        // The alert slides in. A click while it moves can miss the button.
        wait_until_still(window, "keep-working".into(), cx);
        window.click("keep-working", cx);
        window.render_frame(cx);
        assert!(!owner.upgrade().unwrap().read(cx).quit_warning_open);
        assert!(Root::read(window, cx).dialog_focus_handle().is_none());
        assert_eq!(window.focused(cx), original_focus);
        // A new active export can still prompt after declining the first quit.
        let guard = jobs.register(Arc::new(AtomicBool::new(false)));
        owner
            .update(cx, |qrow, cx| qrow.request_quit(window, cx))
            .unwrap();
        window.render_frame(cx);
        assert!(window.try_find("keep-working").is_some());
        window.click("keep-working", cx);
        drop(guard);
    })
    .unwrap();
}

#[gpui_kit::test]
fn later_copy_requests_and_external_clipboard_changes_reject_old_results(cx: &mut TestAppContext) {
    cx.update(|cx| {
        copy_text("before".into(), cx);
        let first = CopyRequest::new(cx);
        let second = CopyRequest::new(cx);
        assert!(!first.is_current(cx));
        assert!(second.is_current(cx));
        copy_text("cell".into(), cx);
        assert!(!second.is_current(cx));
        let external = CopyRequest::new(cx);
        cx.write_to_clipboard(ClipboardItem::new_string("another app".into()));
        assert!(!external.is_current(cx));
        cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        let empty = CopyRequest::new(cx);
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            vec![1, 2, 3],
        )));
        assert!(!empty.is_current(cx));
        let image = CopyRequest::new(cx);
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            vec![4, 5, 6],
        )));
        assert!(!image.is_current(cx));
    });
}
