use gpui_kit::component::{ActiveTheme, h_flex, kbd::Kbd, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;

use super::log_row::PREVIEW_LINES;

/// The longest line that a code block reads. A longer line is cut, so that a
/// statement on one very long line stays cheap to show.
const CODE_LINE_CHARS: usize = 160;
/// The spaces that replace a tab in a code block.
const TAB_WIDTH: usize = 4;

/// The start of a code sample, shown as a code block below the title.
#[derive(Clone, PartialEq)]
struct CodeBlock {
    /// The first lines, without blank lines around them or a shared indent.
    lines: Vec<SharedString>,
    /// More lines follow the shown ones, so the block fades out.
    faded: bool,
    font_family: SharedString,
}

impl CodeBlock {
    /// The first lines of `code`. Only these lines are read.
    fn new(code: &str, font_family: SharedString) -> Self {
        let mut rest = code
            .lines()
            .skip_while(|line| line.trim().is_empty())
            .map(|line| {
                let mut short = String::new();
                for (index, c) in line.chars().enumerate() {
                    if index == CODE_LINE_CHARS {
                        short.push('…');
                        break;
                    }
                    match c {
                        '\t' => short.extend([' '; TAB_WIDTH]),
                        c => short.push(c),
                    }
                }
                short.trim_end().to_owned()
            });
        let mut lines: Vec<String> = rest.by_ref().take(PREVIEW_LINES).collect();
        // A faded block keeps all its lines, so the fade covers its last two.
        let faded = rest.any(|line| !line.is_empty());
        while !faded && lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        let indent = lines
            .iter()
            .filter(|line| !line.is_empty())
            .map(|line| line.len() - line.trim_start_matches(' ').len())
            .min()
            .unwrap_or(0);
        Self {
            lines: lines
                .into_iter()
                .map(|line| line.get(indent..).unwrap_or_default().to_owned().into())
                .collect(),
            faded,
            font_family,
        }
    }

    /// The shown lines as one text.
    fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// The object name leads. The keyboard shortcut, if any, is on the right of
/// the title. Status, detail, and connection metadata are secondary lines
/// below the title. A tooltip without secondary lines, such as a command name
/// with only its shortcut, looks like a plain GPUI Kit tooltip.
#[derive(Clone, PartialEq)]
pub(super) struct StatusTooltip {
    title: SharedString,
    status: SharedString,
    detail: Option<SharedString>,
    code: Option<CodeBlock>,
    metadata: Vec<(SharedString, SharedString)>,
    key_context: Option<SharedString>,
    shortcut_focus: Option<FocusHandle>,
}

impl FluentBuilder for StatusTooltip {}

impl StatusTooltip {
    pub(super) fn new(title: impl Into<SharedString>, status: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            status: status.into(),
            detail: None,
            code: None,
            metadata: Vec::new(),
            key_context: None,
            shortcut_focus: None,
        }
    }

    pub(super) fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Show the start of `code` in a code block in `font_family`.
    pub(super) fn code(mut self, code: &str, font_family: impl Into<SharedString>) -> Self {
        let block = CodeBlock::new(code, font_family.into());
        self.code = (!block.lines.is_empty()).then_some(block);
        self
    }

    pub(super) fn metadata(
        mut self,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        self.metadata.push((label.into(), value.into()));
        self
    }

    fn has_secondary_lines(&self) -> bool {
        !self.status.is_empty()
            || self.detail.is_some()
            || self.code.is_some()
            || !self.metadata.is_empty()
    }

    /// Medium weight sets the title apart from the lines below it only.
    fn title_weight(&self) -> FontWeight {
        if self.has_secondary_lines() {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        }
    }

    /// Resolve the shortcut from the command's binding, including dialog
    /// contexts. Use the same aligned header as status tooltips.
    pub(super) fn for_action(
        mut self,
        action: &dyn Action,
        context: Option<&str>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        self.key_context = context.map(SharedString::new);
        let action = action.boxed_clone();
        move |window, cx| self.build(Some(action.as_ref()), window, cx)
    }

    pub(super) fn for_action_in(
        mut self,
        action: &dyn Action,
        focus: FocusHandle,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        self.shortcut_focus = Some(focus);
        self.for_action(action, None)
    }

    pub(super) fn build(
        &self,
        action: Option<&dyn Action>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyView {
        let content = self.clone();
        let shortcut = action
            .and_then(|action| {
                if let Some(focus) = &self.shortcut_focus {
                    return window.highest_precedence_binding_for_action_in(action, focus);
                }
                window.highest_precedence_binding_for_action_in_context(
                    action,
                    self.key_context
                        .as_deref()
                        .and_then(|context| KeyContext::parse(context).ok())
                        .unwrap_or_default(),
                )
            })
            .and_then(|binding| {
                binding
                    .keystrokes()
                    .first()
                    .map(|key| key.as_keystroke().clone())
            });
        let title_weight = self.title_weight();
        let secondary_lines = self.has_secondary_lines();
        Tooltip::element(move |window, cx| {
            let secondary = |id, text: SharedString| {
                div()
                    .id(id)
                    .test_support()
                    .aria_label(text.clone())
                    .text_xs()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(cx.theme().secondary_foreground)
                    .child(text)
            };
            let mut title = div()
                .id("status-tooltip-title")
                .test_support()
                .flex_1()
                .min_w_0()
                .aria_label(content.title.clone())
                .text_sm()
                .font_weight(title_weight)
                .child(content.title.clone());
            let trailing = shortcut.clone().map(|shortcut| {
                let label: SharedString = Kbd::format(&shortcut).into();
                let element = div()
                    .id("status-tooltip-shortcut")
                    .test_support()
                    .flex_shrink_0()
                    .aria_label(label.clone())
                    .text_xs()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(cx.theme().secondary_foreground)
                    .child(Kbd::new(shortcut).appearance(false));
                (label, element)
            });
            let (title, trailing) = match trailing {
                Some((text, mut element)) => {
                    let title_center =
                        first_line_center(&content.title, &title.style().text, window, cx);
                    let trailing_center =
                        first_line_center(&text, &element.style().text, window, cx);
                    let center = title_center.max(trailing_center);
                    (
                        title.mt(center - title_center),
                        Some(element.mt(center - trailing_center)),
                    )
                }
                None => (title, None),
            };
            v_flex()
                .id("status-tooltip")
                .test_support()
                // Code lines do not wrap, so a code block gets more width.
                .map(|tooltip| match content.code {
                    Some(_) => tooltip.max_w_128(),
                    None => tooltip.max_w_96(),
                })
                .whitespace_normal()
                .gap_0p5()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_start()
                        .justify_between()
                        .gap_3()
                        .child(title)
                        .children(trailing),
                )
                .when(!content.status.is_empty(), |tooltip| {
                    tooltip.child(secondary("status-tooltip-status", content.status.clone()))
                })
                .when_some(content.detail.clone(), |tooltip, detail| {
                    tooltip.child(secondary("status-tooltip-detail", detail))
                })
                .when_some(content.code.clone(), |tooltip, code| {
                    tooltip.child(code_block(code, window, cx))
                })
                .when(!content.metadata.is_empty(), |tooltip| {
                    tooltip.child(
                        v_flex()
                            .id("status-tooltip-metadata")
                            .test_support()
                            .gap_0()
                            .children(content.metadata.iter().map(|(label, value)| {
                                h_flex()
                                    .id(SharedString::from(format!("status-tooltip-{label}")))
                                    .test_support()
                                    .aria_label(format!("{label}: {value}"))
                                    .items_start()
                                    .gap_3()
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .child(
                                        div()
                                            .w_12()
                                            .flex_shrink_0()
                                            .text_color(cx.theme().secondary_foreground)
                                            .child(label.clone()),
                                    )
                                    .child(div().min_w_0().flex_1().child(value.clone()))
                            })),
                    )
                })
        })
        // A single line keeps the padding of a plain tooltip.
        .when(secondary_lines, |tooltip| tooltip.py_1())
        .build(window, cx)
    }

    /// GPUI keeps an open tooltip across trigger renders. Observe its owner
    /// so the title and status follow changes without another pointer move.
    pub(super) fn live<T: 'static>(
        owner: WeakEntity<T>,
        action: Option<&dyn Action>,
        content: impl Fn(&T, &App) -> Option<Self> + 'static,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let action = action.map(|action| Rc::new(action.boxed_clone()));
        let content = Rc::new(content);
        move |_, cx| {
            cx.new(|cx| LiveStatusTooltip {
                _subscription: owner
                    .upgrade()
                    .map(|owner| cx.observe(&owner, |_, _, cx| cx.notify())),
                owner: owner.clone(),
                action: action.clone(),
                content: content.clone(),
                shown: None,
            })
            .into()
        }
    }
}

/// A code block like the SQL blocks in Logs. Lines that do not fit are cut
/// with an ellipsis. A block with more lines fades out at its end.
fn code_block(code: CodeBlock, window: &Window, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    // The code text is extra small; the fade covers two of its lines.
    let size = rems(0.75).to_pixels(window.rem_size());
    let line_height = window
        .text_style()
        .line_height
        .to_pixels(size.into(), window.rem_size());
    v_flex()
        .id("status-tooltip-code")
        .test_support()
        .aria_label(code.text())
        .relative()
        .mt_1()
        .mb_0p5()
        .px_2()
        .py_1()
        .min_w_0()
        .overflow_hidden()
        .rounded(theme.radius)
        .bg(theme.muted)
        .font_family(code.font_family.clone())
        .text_xs()
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.foreground)
        .children(code.lines.iter().map(|line| {
            // An empty line keeps its height.
            div().min_w_0().truncate().child(if line.is_empty() {
                SharedString::new_static(" ")
            } else {
                line.clone()
            })
        }))
        .when(code.faded, |block| {
            block.child(
                div()
                    .id("status-tooltip-code-fade")
                    .test_support()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(line_height * 2.)
                    .rounded_b(theme.radius)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(theme.muted.opacity(0.), 0.),
                        linear_color_stop(theme.muted, 1.),
                    )),
            )
        })
}

/// Center smaller status and shortcut text on the title's first line. Use
/// capital-letter centers, so descenders and wrapped titles do not move the
/// alignment. Snap the painted baseline and center before taking their
/// difference, keeping the correction in whole device pixels at every scale.
fn first_line_center(
    text: &SharedString,
    refinement: &TextStyleRefinement,
    window: &mut Window,
    cx: &App,
) -> Pixels {
    let mut style = window.text_style();
    style.font_family = cx.theme().font_family.clone();
    style.refine(refinement);
    let first: SharedString = text
        .split('\n')
        .next()
        .unwrap_or_default()
        .to_owned()
        .into();
    let size = style.font_size.to_pixels(window.rem_size());
    let line =
        window
            .text_system()
            .shape_line(first.clone(), size, &[style.to_run(first.len())], None);
    let line_height =
        window.pixel_snap(style.line_height.to_pixels(size.into(), window.rem_size()));
    let baseline = window.pixel_snap((line_height - line.ascent - line.descent) / 2. + line.ascent);
    let font = window.text_system().resolve_font(&style.font());
    let cap_height = window.text_system().cap_height(font, size);
    window.pixel_snap(baseline - cap_height / 2.)
}

type TooltipContent<T> = Rc<dyn Fn(&T, &App) -> Option<StatusTooltip>>;

struct LiveStatusTooltip<T> {
    owner: WeakEntity<T>,
    action: Option<Rc<Box<dyn Action>>>,
    content: TooltipContent<T>,
    shown: Option<(StatusTooltip, AnyView)>,
    _subscription: Option<Subscription>,
}

impl<T: 'static> Render for LiveStatusTooltip<T> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self
            .owner
            .upgrade()
            .and_then(|owner| (self.content)(owner.read(cx), cx));
        match content {
            Some(content) => {
                if self
                    .shown
                    .as_ref()
                    .is_none_or(|(shown, _)| *shown != content)
                {
                    let action = self.action.as_ref().map(|action| action.as_ref().as_ref());
                    let view = content.build(action, window, cx);
                    self.shown = Some((content, view));
                }
                div().children(self.shown.as_ref().map(|(_, view)| view.clone()))
            }
            None => {
                self.shown = None;
                div()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CODE_LINE_CHARS, CodeBlock, PREVIEW_LINES, StatusTooltip};
    use gpui_kit::FontWeight;

    fn block(code: &str) -> (String, bool) {
        let block = CodeBlock::new(code, "Mono".into());
        (block.text(), block.faded)
    }

    #[test]
    fn a_code_block_keeps_the_lines_and_shows_only_the_start() {
        // Blank lines around the code and the shared indent go.
        assert_eq!(
            block("\n  \n    SELECT a,\n\t  b\n\n    FROM t  \n\n"),
            ("SELECT a,\n  b\n\nFROM t".into(), false)
        );
        assert_eq!(block(" \n\t\n"), (String::new(), false));
        // Only the first lines show. A block with more lines fades.
        let long = (1..=PREVIEW_LINES + 1)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let shown = (1..=PREVIEW_LINES)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(block(&long), (shown.clone(), true));
        assert_eq!(block(&format!("{shown}\n\n  \n")), (shown, false));
        // A faded block keeps blank lines, so the fade covers its last lines.
        let gap = format!("a{}\nb", "\n".repeat(PREVIEW_LINES));
        let gap = CodeBlock::new(&gap, "Mono".into());
        assert!(gap.faded);
        assert_eq!(gap.lines.len(), PREVIEW_LINES);
        // A very long line is cut, also in multibyte text.
        let wide = "日😀".repeat(CODE_LINE_CHARS);
        let (text, _) = block(&wide);
        assert_eq!(text.chars().count(), CODE_LINE_CHARS + 1);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn a_code_block_is_a_secondary_line() {
        assert_eq!(
            StatusTooltip::new("Result", "")
                .code("SELECT 1", "Mono")
                .title_weight(),
            FontWeight::MEDIUM
        );
        // Code without text adds no block.
        assert_eq!(
            StatusTooltip::new("Result", "")
                .code("\n  \n", "Mono")
                .title_weight(),
            FontWeight::NORMAL
        );
    }

    #[test]
    fn only_a_title_with_secondary_lines_is_medium_weight() {
        assert_eq!(
            StatusTooltip::new("Maximize", "").title_weight(),
            FontWeight::NORMAL
        );
        assert_eq!(
            StatusTooltip::new("Pinned result", "Pinned").title_weight(),
            FontWeight::MEDIUM
        );
        assert_eq!(
            StatusTooltip::new("Result", "")
                .detail("Detail")
                .title_weight(),
            FontWeight::MEDIUM
        );
        assert_eq!(
            StatusTooltip::new("Connection", "")
                .metadata("Host", "db")
                .title_weight(),
            FontWeight::MEDIUM
        );
    }
}
