use super::*;
use gpui_kit::component::notification::Notification;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

impl Qrow {
    pub(super) fn import_sql_file(
        &mut self,
        _: &ImportSqlFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        let Some(profile) = self.active_profile() else {
            window.push_notification(
                Notification::error("Choose a connection before importing an SQL file."),
                cx,
            );
            return;
        };
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match prompt.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => return,
                error => {
                    let _ = this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error(format!(
                                "Could not open the file panel: {error:?}"
                            )),
                            cx,
                        );
                    });
                    return;
                }
            };
            let files = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .take(1)
                        .map(|path| {
                            let sql = std::fs::read_to_string(&path);
                            (path, sql)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !this
                    .profiles
                    .iter()
                    .any(|candidate| candidate.id == profile)
                {
                    return;
                }
                for (path, sql) in files {
                    match sql {
                        Ok(sql) => {
                            let title = path
                                .file_stem()
                                .and_then(|name| name.to_str())
                                .unwrap_or("Query");
                            let title: String = title
                                .chars()
                                .filter(|ch| !ch.is_control())
                                .take(MAX_TAB_TITLE)
                                .collect();
                            let title = if title.trim().is_empty() {
                                "Query"
                            } else {
                                title.trim()
                            };
                            let mut saved = SavedTab::new(1, Some(profile));
                            saved.title = unique_tab_title(title, |candidate| {
                                this.tabs.iter().any(|tab| {
                                    tab.saved.profile == Some(profile)
                                        && tab.saved.title == candidate
                                })
                            });
                            saved.sql = sql.strip_prefix('\u{feff}').unwrap_or(&sql).to_owned();
                            let tab = this.make_tab(saved, window, cx);
                            this.tabs.push(tab);
                            this.activate(this.tabs.len() - 1, window, cx);
                        }
                        Err(error) => window.push_notification(
                            Notification::error(format!(
                                "Could not import {}: {error}",
                                path.display()
                            )),
                            cx,
                        ),
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn save_as_sql_file(
        &mut self,
        _: &SaveAsSqlFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_sql_tab(self.tabs[self.active].saved.id, window, cx);
    }

    pub(super) fn save_sql_tab(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        let Some(tab) = self.tabs.iter().find(|tab| tab.saved.id == id) else {
            return;
        };
        let sql = tab.input.read(cx).value().to_string();
        let title: String = tab
            .saved
            .title
            .chars()
            .map(|ch| {
                if ch.is_control() || matches!(ch, '/' | '\\' | ':') {
                    '_'
                } else {
                    ch
                }
            })
            .collect();
        let filename = format!(
            "{}.sql",
            title.trim().strip_suffix(".sql").unwrap_or(title.trim())
        );
        let directory = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
        let prompt = cx.prompt_for_new_path(&directory, Some(&filename));
        cx.spawn_in(window, async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                error => {
                    let _ = this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error(format!(
                                "Could not open the save panel: {error:?}"
                            )),
                            cx,
                        )
                    });
                    return;
                }
            };
            let result = cx
                .background_spawn(async move { save_sql(&path, &sql).map(|()| path) })
                .await;
            let _ = this.update_in(cx, |_, window, cx| match result {
                Ok(path) => window.push_notification(saved_notification(path), cx),
                Err(error) => window.push_notification(
                    Notification::error(format!("Could not save SQL file: {error}")),
                    cx,
                ),
            });
        })
        .detach();
    }
}

fn save_sql(path: &Path, sql: &str) -> std::io::Result<()> {
    crate::export::save(path, &AtomicBool::new(false), |out| {
        out.write_all(sql.as_bytes())
    })
}

/// A routine save hides on its own; hovering the notification pauses the timer.
fn saved_notification(path: PathBuf) -> Notification {
    super::file_notice::saved_file(
        super::file_notice::SavedFileIds {
            notice: "sql-file-saved-notice",
            title: "sql-file-saved-detail",
            filename: "sql-file-saved-filename",
            reveal: "reveal-sql-file",
        },
        "SQL file saved".into(),
        path,
    )
    .autohide(true)
}
