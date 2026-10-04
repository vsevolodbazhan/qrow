//! Launches the real Qrow window headlessly with an isolated workspace and
//! synthetic passwords. Real worker threads do real I/O, so waits use wall time.
//! The `ui` and `e2e` test binaries share this module; each uses a part of it.
#![allow(dead_code)]

pub mod assistant;
pub mod evidence;
pub mod fixture;
pub mod perf;
use anyhow::Result;
use gpui_kit::InputEvent as _;
use gpui_kit::test::ElementSnapshot;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext, Bounds, ClipboardItem, ElementId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, TestAppContext, WeakEntity,
    Window, point, px, size,
};
use qrow::{
    model::{Profile, WORKSPACE_VERSION, Workspace},
    storage::{self, Credentials},
    ui::{self, Environment, Qrow},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
use uuid::Uuid;
use zeroize::Zeroizing;

/// Synthetic passwords. Records reads so tests can prove Keychain was not used.
#[derive(Default)]
pub struct MemoryCredentials {
    passwords: Mutex<HashMap<Uuid, String>>,
    reads: AtomicUsize,
}

impl MemoryCredentials {
    pub fn get(&self, id: Uuid) -> Option<String> {
        self.passwords.lock().unwrap().get(&id).cloned()
    }
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
    pub fn count(&self) -> usize {
        self.passwords.lock().unwrap().len()
    }
}

impl Credentials for MemoryCredentials {
    fn password(&self, id: Uuid) -> Result<Zeroizing<String>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.get(id)
            .map(Zeroizing::new)
            .ok_or_else(|| anyhow::anyhow!("No synthetic password for {id}"))
    }
    fn set_password(&self, id: Uuid, password: &str) -> Result<()> {
        self.passwords.lock().unwrap().insert(id, password.into());
        Ok(())
    }
    fn delete_password(&self, id: Uuid) -> Result<()> {
        self.passwords.lock().unwrap().remove(&id);
        Ok(())
    }
}

/// The observed element `id` inside the scope `scope`, when both exist in the
/// last frame. `TestWindowExt::within` panics when its scope is absent.
pub fn find_in(
    window: &Window,
    scope: impl Into<ElementId>,
    id: impl Into<ElementId>,
) -> Option<ElementSnapshot> {
    let (scope, id) = (scope.into(), id.into());
    gpui_kit::base::test_support::snapshots(window)
        .into_iter()
        .find(|element| element.path().last() == Some(&id) && element.path().contains(&scope))
}

/// Whether `id` is an observed element, or the scope of one, in the last frame.
pub fn present(window: &Window, id: &ElementId) -> bool {
    window.try_find(id.clone()).is_some()
        || elements(window)
            .iter()
            .any(|element| element.path().contains(id))
}

/// Every observed element of the last frame.
pub fn elements(window: &Window) -> Vec<ElementSnapshot> {
    gpui_kit::base::test_support::snapshots(window)
}

/// Compare the first text baselines, rather than the bottoms of their boxes.
pub fn assert_tooltip_header_baseline(window: &mut Window, cx: &App, secondary: &str) {
    use gpui_kit::component::ActiveTheme;
    use gpui_kit::{FontWeight, Styled, div};

    let mut title = div().text_sm().font_weight(FontWeight::MEDIUM);
    let mut secondary_style = div().text_xs();
    let baseline = |id: &str, font_size, weight, window: &mut Window| {
        let mut style = window.text_style();
        style.font_family = cx.theme().font_family.clone();
        style.font_size = font_size;
        style.font_weight = weight;
        let text = label(window, id.to_owned()).unwrap();
        let first = text.lines().next().unwrap_or_default().to_owned();
        let size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(
            first.clone().into(),
            size,
            &[style.to_run(first.len())],
            None,
        );
        let line_height =
            window.pixel_snap(style.line_height.to_pixels(size.into(), window.rem_size()));
        bounds_of(window, id).top() + (line_height - line.ascent - line.descent) / 2. + line.ascent
    };
    let title_baseline = baseline(
        "status-tooltip-title",
        title.style().text.font_size.unwrap(),
        FontWeight::MEDIUM,
        window,
    );
    let secondary_baseline = baseline(
        secondary,
        secondary_style.style().text.font_size.unwrap(),
        FontWeight::NORMAL,
        window,
    );
    assert_eq!(
        window.pixel_snap(title_baseline),
        window.pixel_snap(secondary_baseline),
        "{secondary} painted baseline differs from the title"
    );
}

/// The labels of every observed element, for failure messages.
pub fn labels(window: &Window) -> Vec<String> {
    let mut labels: Vec<_> = elements(window)
        .iter()
        .filter_map(|element| element.label().map(str::to_owned))
        .filter(|label| !label.is_empty())
        .collect();
    labels.sort();
    labels
}

/// Whether an observed element has a label that contains `text`.
pub fn shows(window: &Window, text: &str) -> bool {
    elements(window)
        .iter()
        .any(|element| element.label().is_some_and(|label| label.contains(text)))
}

/// The label of the observed element `id`.
pub fn label(window: &Window, id: impl Into<ElementId>) -> Option<String> {
    window
        .try_find(id)
        .and_then(|element| element.label().map(str::to_owned))
}

/// The accessibility value of the observed element `id`.
pub fn value(window: &Window, id: impl Into<ElementId>) -> Option<String> {
    window
        .try_find(id)
        .and_then(|element| element.value().map(str::to_owned))
}

/// The index of the item labelled `item` in the open menu `scope`
/// ("popup-menu" or "submenu").
pub fn menu_item(window: &mut Window, scope: &str, item: &str) -> Option<usize> {
    if window.try_find(scope.to_owned()).is_none()
        && find_in(window, scope.to_owned(), 0usize).is_none()
    {
        return None;
    }
    let scoped = window.within(scope.to_owned());
    (0..64).find(|index| {
        scoped
            .try_find(*index)
            .is_some_and(|entry| entry.label() == Some(item))
    })
}

/// The observed element labelled exactly `text`.
pub fn labelled(window: &Window, text: &str) -> Option<ElementSnapshot> {
    elements(window)
        .into_iter()
        .find(|element| element.label() == Some(text))
}

/// The observed elements whose labels start with `prefix`.
pub fn labelled_starting(window: &Window, prefix: &str) -> Vec<ElementSnapshot> {
    elements(window)
        .into_iter()
        .filter(|element| {
            element
                .label()
                .is_some_and(|label| label.starts_with(prefix))
        })
        .collect()
}

/// The bounds of the observed element `id`, or of the element labelled
/// `id` when no element has that ID.
pub fn bounds_of(window: &Window, id: &str) -> Bounds<Pixels> {
    window
        .try_find(id.to_owned())
        .or_else(|| labelled(window, id))
        .unwrap_or_else(|| panic!("No element {id}. Labels: {:?}", labels(window)))
        .bounds()
}

/// One painted dot in a region, in the expected theme color.
fn assert_dot_in(window: &Window, bounds: Bounds<Pixels>, expected: Option<gpui_kit::Hsla>) {
    let bounds = bounds.scale(window.scale_factor());
    let dot_size = px(6.).scale(window.scale_factor());
    let dots: Vec<_> = window
        .painted_quads()
        .into_iter()
        .filter(|quad| {
            quad.bounds.size.width == dot_size
                && quad.bounds.size.height == dot_size
                && bounds.contains(&quad.bounds.center())
                && quad.content_mask.bounds.contains(&quad.bounds.center())
        })
        .collect();
    assert_eq!(dots.len(), usize::from(expected.is_some()), "{dots:?}");
    if let Some(color) = expected {
        assert_eq!(dots[0].background, gpui_kit::Background::from(color));
    }
}

/// A tab paints one status dot and keeps its Close control visible.
pub fn assert_tab_dot(window: &Window, tab: Uuid, expected: Option<gpui_kit::Hsla>) {
    let close = window.find(format!("close-tab-{tab}"));
    assert!(close.visible(), "The Close control is hidden");
    let tab = elements(window)
        .into_iter()
        .find(|element| {
            element.role() == Some(gpui_kit::Role::Tab)
                && element.bounds().contains(&close.bounds().center())
        })
        .expect("The Close control is inside a tab");
    assert_dot_in(window, tab.bounds(), expected);
}

/// A connection row paints one dot in the expected theme color.
pub fn assert_connection_dot(window: &Window, profile: Uuid, expected: gpui_kit::Hsla) {
    let dot = window.find(format!("connection-status-{profile}"));
    assert!(dot.visible(), "The connection dot is hidden");
    let row = elements(window)
        .into_iter()
        .find(|element| {
            element.role() == Some(gpui_kit::Role::TreeItem)
                && element.bounds().contains(&dot.bounds().center())
        })
        .expect("The connection dot is inside a tree row");
    assert_dot_in(window, row.bounds(), Some(expected));
}

/// Clicks the center of an observed element, as `TestWindowExt::click` does
/// for an ID. Use it for elements that GPUI Kit owns and does not name, like
/// the search field of Settings. Prefer IDs for Qrow's own controls.
pub fn click_element(window: &mut Window, element: &ElementSnapshot, cx: &mut App) {
    pointer_click(window, element, MouseButton::Left, cx);
}

/// Presses and releases `button` at the center of an observed element.
pub fn pointer_click(
    window: &mut Window,
    element: &ElementSnapshot,
    button: MouseButton,
    cx: &mut App,
) {
    assert!(element.visible(), "{:?} is not visible", element.path());
    let position = element.bounds().center();
    window.dispatch_event(
        MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers: Default::default(),
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        MouseDownEvent {
            button,
            position,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        MouseUpEvent {
            button,
            position,
            modifiers: Default::default(),
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
}

/// The label of a result cell. Column 0 holds the row number.
pub fn cell(window: &Window, row: usize, column: usize) -> Option<String> {
    find_in(window, ("row", row), ("cell", column)).and_then(|cell| cell.label().map(str::to_owned))
}

/// The label of a result column header. Column 0 is the row number column.
pub fn header(window: &Window, column: usize) -> Option<String> {
    window
        .try_find(("column-header", column))
        .and_then(|header| header.label().map(str::to_owned))
}

pub struct TestApp {
    pub window: AnyWindowHandle,
    /// The view of the window, for state that no control reaches, like the
    /// in-memory Activity of a performance probe. The window owns the view,
    /// so a closed window releases it and its workspace lock.
    pub qrow: WeakEntity<Qrow>,
    pub credentials: Arc<MemoryCredentials>,
    workspace: PathBuf,
    _directory: TempDir,
}

impl TestApp {
    /// Opens Qrow on `workspace`, saved to a new temporary directory.
    pub fn launch(cx: &mut TestAppContext, workspace: Workspace) -> Self {
        Self::launch_with(cx, workspace, MemoryCredentials::default())
    }

    pub fn launch_with(
        cx: &mut TestAppContext,
        workspace: Workspace,
        credentials: MemoryCredentials,
    ) -> Self {
        Self::launch_in(cx, tempfile::tempdir().unwrap(), workspace, credentials)
    }

    /// Opens Qrow with its built-in demo data, which saves nothing.
    pub fn launch_demo(cx: &mut TestAppContext) -> Self {
        Self::open(
            cx,
            tempfile::tempdir().unwrap(),
            None,
            MemoryCredentials::default(),
        )
    }

    /// Opens Qrow on `workspace`, saved in `directory`.
    pub fn launch_in(
        cx: &mut TestAppContext,
        directory: TempDir,
        workspace: Workspace,
        credentials: MemoryCredentials,
    ) -> Self {
        Self::open(cx, directory, Some(workspace), credentials)
    }

    /// Opens Qrow on `workspace` in `directory`, or on the demo data.
    fn open(
        cx: &mut TestAppContext,
        directory: TempDir,
        workspace: Option<Workspace>,
        credentials: MemoryCredentials,
    ) -> Self {
        // Worker and saver threads wake the UI. GPUI's deterministic
        // scheduler rejects wakes from other threads unless parking is allowed.
        cx.executor().allow_parking();
        cx.update(|cx| {
            ui::init(cx);
            // GPUI animations use wall time, which the test clock does not
            // control. Reduced motion settles dialogs on their first frame.
            cx.set_reduce_motion(true);
        });
        let path = directory.path().join("workspace.json");
        let credentials = Arc::new(credentials);
        let environment = match workspace {
            Some(workspace) => {
                let workspace = Workspace {
                    version: WORKSPACE_VERSION,
                    ..workspace
                };
                std::fs::write(&path, serde_json::to_vec(&workspace).unwrap()).unwrap();
                Environment::isolated(path.clone(), credentials.clone())
            }
            None => Environment::demo(),
        };
        let mut qrow = None;
        let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
            let view = cx.new(|cx| Qrow::new(environment, Instant::now(), window, cx));
            qrow = Some(view.downgrade());
            ui::root(view, window, cx)
        });
        let app = Self {
            window: window.into(),
            qrow: qrow.expect("The window has a Qrow view"),
            credentials,
            workspace: path,
            _directory: directory,
        };
        app.update(cx, |window, cx| window.render_frame(cx));
        app
    }

    pub fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Window, &mut App) -> R,
    ) -> R {
        cx.update_window(self.window, |_, window, cx| f(window, cx))
            .expect("Qrow window closed")
    }

    pub fn click(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        self.update(cx, |window, cx| window.click(id, cx));
    }

    pub fn press(&self, cx: &mut TestAppContext, key: &str) {
        self.update(cx, |window, cx| window.press(key, cx));
    }

    /// Dispatches an action as a key binding or a menu item does.
    pub fn dispatch(&self, cx: &mut TestAppContext, action: impl Action) {
        self.update(cx, |window, cx| {
            window.dispatch_action(action.boxed_clone(), cx)
        });
    }

    /// Waits until the element `id` is in the frame.
    pub fn wait_for(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        let what = format!("{id:?}");
        self.wait_until(cx, &what, Duration::from_secs(10), |window, _| {
            present(window, &id)
        });
    }

    /// Waits until the element `id` has left the frame.
    pub fn wait_gone(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        let what = format!("{id:?} to close");
        self.wait_until(cx, &what, Duration::from_secs(10), |window, _| {
            !present(window, &id)
        });
    }

    /// Clicks the element labelled exactly `text`.
    pub fn click_labelled(&self, cx: &mut TestAppContext, text: &str) {
        self.wait_until(cx, text, Duration::from_secs(10), |window, _| {
            labelled(window, text).is_some()
        });
        self.update(cx, |window, cx| {
            let element = labelled(window, text).unwrap();
            click_element(window, &element, cx);
        });
    }

    /// Replaces the text of the focused input after clicking the element
    /// labelled `text`.
    pub fn fill_labelled(&self, cx: &mut TestAppContext, text: &str, value: &str) {
        self.click_labelled(cx, text);
        self.update(cx, |window, cx| {
            window.press("cmd-a", cx);
            if value.is_empty() {
                window.press("backspace", cx);
            } else {
                window.input(value, cx);
            }
        });
    }

    /// Opens the select `id` and chooses `option`. A searchable select filters
    /// to the option; another select moves down its list with the keyboard.
    /// Each attempt confirms the next option, and the list opens at the
    /// confirmed option, so one step down per attempt reaches every option.
    pub fn select(&self, cx: &mut TestAppContext, id: &str, option: &str) {
        let chosen = |app: &Self, cx: &mut TestAppContext| {
            app.settle(cx);
            app.update(cx, |window, _| {
                value(window, id.to_owned()).as_deref() == Some(option)
            })
        };
        self.update(cx, |window, cx| {
            window.within(id.to_owned()).click("input", cx)
        });
        self.settle(cx);
        self.update(cx, |window, cx| window.input(option, cx));
        self.settle(cx);
        self.press(cx, "enter");
        if chosen(self, cx) {
            return;
        }
        for _ in 1..10 {
            self.update(cx, |window, cx| {
                window.within(id.to_owned()).click("input", cx)
            });
            self.settle(cx);
            self.press(cx, "down");
            self.press(cx, "enter");
            if chosen(self, cx) {
                return;
            }
        }
        panic!("{id} has no option {option}");
    }

    /// Runs the queued work of the UI, like deferred updates of a select.
    pub fn settle(&self, cx: &mut TestAppContext) {
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        self.update(cx, |window, cx| window.render_frame(cx));
    }

    /// The value of the stepper `id` of Settings.
    pub fn stepper(&self, cx: &mut TestAppContext, id: &str) -> String {
        self.update(cx, |window, _| {
            window
                .within(id.to_owned())
                .find("value")
                .value()
                .unwrap_or_default()
                .to_owned()
        })
    }

    /// Clicks `button` ("increment" or "decrement") of the stepper `id` and
    /// waits for `expected`.
    pub fn step(&self, cx: &mut TestAppContext, id: &str, button: &str, expected: &str) {
        self.update(cx, |window, cx| {
            window.within(id.to_owned()).click(button.to_owned(), cx)
        });
        self.wait_until(
            cx,
            &format!("{id} to show {expected}"),
            Duration::from_secs(10),
            |window, _| {
                window
                    .within(id.to_owned())
                    .try_find("value")
                    .and_then(|v| v.value().map(str::to_owned))
                    .as_deref()
                    == Some(expected)
            },
        );
    }

    /// Right-clicks the element labelled exactly `text` and waits for its menu.
    pub fn context_menu_labelled(&self, cx: &mut TestAppContext, text: &str) {
        self.wait_until(cx, text, Duration::from_secs(10), |window, _| {
            labelled(window, text).is_some()
        });
        self.update(cx, |window, cx| {
            let element = labelled(window, text).unwrap();
            pointer_click(window, &element, MouseButton::Right, cx);
        });
        self.wait_for(cx, "popup-menu");
    }

    /// Scrolls the container of `target` with the wheel until `target` is
    /// visible, like a user who scrolls a form to a field below its fold or
    /// back to a field above it. The
    /// wheel turns over a visible element of the same container. A snapshot
    /// is visible when any part of it shows, so a target in the lower half of
    /// the window gets one more step, which shows all of it.
    pub fn scroll_to(&self, cx: &mut TestAppContext, target: &str) {
        let mut positions = Vec::new();
        let mut extra_step = true;
        for _ in 0..30 {
            let position = self.update(cx, |window, _| {
                let element = window.try_find(target.to_owned())?;
                let low = element.bounds().center().y > window.viewport_size().height / 2.;
                if element.visible() && !(low && extra_step) {
                    return None;
                }
                if element.visible() {
                    extra_step = false;
                }
                let depth = element
                    .path()
                    .iter()
                    .rposition(|id| format!("{id:?}").contains("Scrollable"))
                    .expect("The target is not in a scroll container");
                let container = &element.path()[..=depth];
                // The visible element nearest the middle of the window is in
                // the scroll area. One at its edge can be under a footer, and
                // a large wrapper can have its center outside the window.
                // Snapshots have no order, so the choice must not depend on
                // it.
                let middle = f32::from(window.viewport_size().height) / 2.;
                elements(window)
                    .into_iter()
                    .filter(|other| other.visible() && other.path().starts_with(container))
                    .min_by(|a, b| {
                        let key = |e: &ElementSnapshot| {
                            (
                                (f32::from(e.bounds().center().y) - middle).abs(),
                                f32::from(e.bounds().size.width)
                                    * f32::from(e.bounds().size.height),
                            )
                        };
                        let (a, b) = (key(a), key(b));
                        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
                    })
                    .map(|other| (other.bounds().center(), low))
            });
            let Some((position, down)) = position else {
                let found =
                    self.update(cx, |window, _| window.try_find(target.to_owned()).is_some());
                assert!(found, "No element {target}");
                return;
            };
            positions.push(position);
            self.update(cx, |window, cx| {
                window.dispatch_event(
                    gpui_kit::ScrollWheelEvent {
                        position,
                        // A target above the middle of the window is above
                        // the visible part of the container.
                        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                            px(0.),
                            px(if down { -120. } else { 120. }),
                        )),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
                window.render_frame(cx);
            });
            self.settle(cx);
        }
        let state = self.update(cx, |window, _| {
            window
                .try_find(target.to_owned())
                .map(|e| format!("bounds {:?}, visible {}", e.bounds(), e.visible()))
        });
        panic!("{target} did not scroll into view: {state:?}. Wheel positions: {positions:?}");
    }

    /// Moves the pointer over the element labelled exactly `text`, for
    /// controls that show on hover, like the close button of a tab.
    pub fn hover_labelled(&self, cx: &mut TestAppContext, text: &str) {
        self.wait_until(cx, text, Duration::from_secs(10), |window, _| {
            labelled(window, text).is_some()
        });
        self.update(cx, |window, cx| {
            let position = labelled(window, text).unwrap().bounds().center();
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
    }

    /// Right-clicks the one element whose label starts with `prefix`.
    pub fn context_menu_starting(&self, cx: &mut TestAppContext, prefix: &str) {
        self.wait_until(cx, prefix, Duration::from_secs(10), |window, _| {
            labelled_starting(window, prefix).len() == 1
        });
        self.update(cx, |window, cx| {
            let element = labelled_starting(window, prefix).remove(0);
            pointer_click(window, &element, MouseButton::Right, cx);
        });
        self.wait_for(cx, "popup-menu");
    }

    /// Clicks the first element whose label starts with `prefix`.
    pub fn click_starting(&self, cx: &mut TestAppContext, prefix: &str) {
        self.wait_until(cx, prefix, Duration::from_secs(10), |window, _| {
            !labelled_starting(window, prefix).is_empty()
        });
        self.update(cx, |window, cx| {
            let element = labelled_starting(window, prefix).remove(0);
            click_element(window, &element, cx);
        });
    }

    /// Closes the window, which saves and releases the workspace, and opens
    /// Qrow again on the same workspace directory.
    pub fn relaunch(self, cx: &mut TestAppContext) -> Self {
        let Self {
            window,
            credentials,
            _directory: directory,
            ..
        } = self;
        cx.update_window(window, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        let copy = MemoryCredentials::default();
        for (id, password) in credentials.passwords.lock().unwrap().iter() {
            copy.set_password(*id, password).unwrap();
        }
        let workspace = storage::load(&directory.path().join("workspace.json")).unwrap();
        Self::launch_in(cx, directory, workspace, copy)
    }

    /// Opens the submenu `parent` of the open menu and clicks its `item`.
    pub fn choose_in_submenu(&self, cx: &mut TestAppContext, parent: &str, item: &str) {
        self.wait_until(cx, parent, Duration::from_secs(10), |window, _| {
            menu_item(window, "popup-menu", parent).is_some()
        });
        self.update(cx, |window, cx| {
            let index = menu_item(window, "popup-menu", parent).unwrap();
            window.within("popup-menu").hover(index, cx);
        });
        self.choose(cx, "submenu", item);
    }

    /// Right-clicks `id` and waits for its context menu.
    pub fn context_menu(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
        let id = id.into();
        self.update(cx, |window, cx| window.right_click(id, cx));
        self.wait_for(cx, "popup-menu");
    }

    /// Clicks the item labelled `item` in the open menu `scope`.
    pub fn choose(&self, cx: &mut TestAppContext, scope: &str, item: &str) {
        self.wait_until(
            cx,
            &format!("{item} in {scope}"),
            Duration::from_secs(10),
            |window, _| menu_item(window, scope, item).is_some(),
        );
        self.update(cx, |window, cx| {
            let index = menu_item(window, scope, item).unwrap();
            window.within(scope.to_owned()).click(index, cx);
        });
    }

    /// Clicks `id`, selects its text, and types `text` in its place. Masked
    /// inputs do not publish their value, so only unmasked values are checked.
    pub fn fill(&self, cx: &mut TestAppContext, id: &'static str, text: &str) {
        self.update(cx, |window, cx| {
            window.click(id, cx);
            window.press("cmd-a", cx);
            if text.is_empty() {
                window.press("backspace", cx);
            } else {
                window.input(text, cx);
            }
            let input = window.find(id);
            assert_eq!(input.focused(), Some(true), "{id} did not take focus");
            if let Some(value) = input.value() {
                assert_eq!(value, text, "{id} did not accept the text");
            }
        });
    }

    pub fn workspace_path(&self) -> &Path {
        &self.workspace
    }

    /// Replaces the SQL of the active tab by typing into the editor, and waits
    /// until Qrow saves it.
    pub fn type_sql(&self, cx: &mut TestAppContext, sql: &str) {
        self.update(cx, |window, cx| {
            window.click("sql-editor", cx);
            window.press("cmd-a", cx);
            window.input(sql, cx);
        });
        self.wait_until(
            cx,
            "the typed SQL to be saved",
            Duration::from_secs(10),
            |_, _| self.saved().tabs.iter().any(|tab| tab.sql == sql),
        );
    }

    /// The workspace as Qrow last saved it.
    /// The text of the Activity of `profile`, as Copy All copies it with
    /// the filter that shows. Activity opens from the connection menu and
    /// closes again.
    pub fn activity(&self, cx: &mut TestAppContext, profile: Uuid) -> String {
        self.context_menu(cx, connection_row(profile));
        self.choose(cx, "popup-menu", "Show Activity");
        self.wait_for(cx, "activity");
        let text = self.copy_activity(cx);
        self.press(cx, "escape");
        self.wait_gone(cx, "activity");
        text
    }

    /// The text that Copy All copies from the open Activity.
    pub fn copy_activity(&self, cx: &mut TestAppContext) -> String {
        cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        self.click(cx, "activity-copy-all");
        self.settle(cx);
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    }

    pub fn saved(&self) -> Workspace {
        storage::load(&self.workspace).unwrap()
    }

    /// Polls `predicate` on fresh frames until it holds. Fails with the
    /// registered element paths after `timeout` of wall time.
    pub fn wait_until(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        timeout: Duration,
        mut predicate: impl FnMut(&mut Window, &mut App) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        loop {
            // Advance the test clock for Qrow's polling timers, then run the
            // tasks that real threads woke.
            cx.executor().advance_clock(Duration::from_millis(50));
            cx.run_until_parked();
            let ready = self.update(cx, |window, cx| {
                window.render_frame(cx);
                predicate(window, cx)
            });
            if ready {
                return;
            }
            if Instant::now() >= deadline {
                let labels = self.update(cx, |window, _| {
                    gpui_kit::base::test_support::snapshots(window)
                        .iter()
                        .filter_map(|element| element.label().map(str::to_owned))
                        .collect::<Vec<_>>()
                });
                let saved = std::fs::read_to_string(&self.workspace).unwrap_or_default();
                panic!(
                    "Timed out after {timeout:?} waiting for {what}.\nLabels: {labels:?}\nSaved workspace: {saved}"
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A synthetic connection that no test connects to. It browses schemas on
/// request, so tests can show a cached schema tree.
pub fn offline_profile(name: &str) -> Profile {
    let mut profile = Profile {
        name: name.into(),
        host: "example.invalid".into(),
        port: 10009,
        username: "synthetic".into(),
        database: "default".into(),
        ..Profile::default()
    };
    profile.catalog.refresh = qrow::model::CatalogRefresh::Manual;
    profile
}

/// Presses the left button at `position` `count` times in a row, like a
/// double-click when `count` is 2.
pub fn press_at(window: &mut Window, position: Point<Pixels>, count: usize, cx: &mut App) {
    for click_count in 1..=count {
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count,
            }
            .to_platform_input(),
            cx,
        );
    }
    window.render_frame(cx);
}

impl TestApp {
    /// Expands or collapses a connection in the schema tree with the
    /// disclosure before its button. A click on the button selects it.
    pub fn toggle_connection(&self, cx: &mut TestAppContext, profile: Uuid) {
        let button = self.update(cx, |window, _| {
            window.find(connection_row(profile)).bounds()
        });
        self.update(cx, |window, cx| {
            press_at(
                window,
                point(button.left() - px(8.), button.center().y),
                1,
                cx,
            )
        });
    }
}

/// The element ID of the sidebar row of a connection.
pub fn connection_row(id: Uuid) -> ElementId {
    ElementId::Name(format!("profile-{id}").into())
}
