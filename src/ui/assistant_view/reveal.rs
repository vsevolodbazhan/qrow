//! The paced reveal of streamed replies. Codex sends a reply in parts of
//! different sizes at irregular times. The transcript shows the received text
//! word by word at an even speed, a short time after it arrives.
use super::*;
use std::time::Duration;

/// The time between two steps of the reveal.
const FRAME: Duration = Duration::from_millis(16);
/// The reveal stays about this many seconds behind the received text.
const LAG: f32 = 0.3;
/// The seconds that the speed takes to follow a change of the received rate.
const SMOOTHING: f32 = 0.3;
/// The lowest speed in characters per second, so that a short end shows soon.
const MIN_RATE: f32 = 60.;
/// The longest step in seconds, so that a late frame does not show a burst.
const MAX_STEP: f32 = 0.1;
/// After this many seconds without new text, the reveal also shows an
/// unfinished word or Markdown span. Codex can pause a reply for a tool call.
const SETTLE: f32 = 0.25;
/// A longer run without spaces, for example CJK text or a URL, shows by
/// characters.
const MAX_WORD: usize = 24;

/// The reveal state of one streamed reply.
#[derive(Clone, Debug, Default)]
pub(super) struct Reveal {
    /// The byte length of the shown part of the text.
    shown: usize,
    /// The speed in characters per second.
    rate: f32,
    /// Characters that the speed allows and that do not show yet.
    credit: f32,
    /// Seconds since the text last became longer.
    waited: f32,
    /// No more text comes.
    complete: bool,
}

impl Reveal {
    pub fn shown(&self) -> usize {
        self.shown
    }

    /// The text became longer.
    pub fn received(&mut self) {
        self.waited = 0.;
    }

    pub fn complete(&mut self) {
        self.complete = true;
    }

    /// Shows all of `text` at once.
    pub fn show_all(&mut self, text: &str) {
        self.shown = text.len();
        self.credit = 0.;
    }

    /// Moves the shown part of `text` forward for `elapsed` time. Returns
    /// whether the shown part changed.
    pub fn advance(&mut self, text: &str, elapsed: Duration) -> bool {
        let step = elapsed.as_secs_f32().min(MAX_STEP);
        self.waited += step;
        if self.shown >= text.len() {
            // The speed stays for the next part.
            self.credit = 0.;
            return false;
        }
        let backlog = text[self.shown..].chars().count() as f32;
        let target = (backlog / LAG).max(MIN_RATE);
        self.rate = if self.rate == 0. {
            target
        } else {
            self.rate + (target - self.rate) * (1. - (-step / SMOOTHING).exp())
        };
        // A held word does not collect a burst of credit.
        self.credit = (self.credit + self.rate * step).min(self.rate * MAX_STEP + 1.);
        if self.credit < 1. {
            return false;
        }
        let settled = self.complete || self.waited >= SETTLE;
        let wanted = text[self.shown..]
            .char_indices()
            .nth(self.credit as usize)
            .map_or(text.len(), |(index, _)| self.shown + index);
        let cut = cut(text, self.shown, wanted, settled);
        if cut <= self.shown {
            return false;
        }
        self.credit -= text[self.shown..cut].chars().count() as f32;
        self.shown = cut;
        true
    }
}

/// The end of the shown part near `wanted`. It does not split a word, an
/// inline Markdown span, or a table row. When `settled`, the end of `text`
/// can also end the shown part.
pub(super) fn cut(text: &str, shown: usize, wanted: usize, settled: bool) -> usize {
    let mut cut = word_end(text, shown, wanted, settled);
    // A span that ends later can end in a word that opens another span.
    for _ in 0..8 {
        let next = span_end(text, shown, cut, settled);
        let next = row_end(text, shown, next, settled);
        let next = if next > cut {
            word_end(text, shown, next, settled)
        } else {
            next
        };
        if next == cut {
            break;
        }
        cut = next;
    }
    cut.max(shown)
}

/// The end of the word at `wanted`.
fn word_end(text: &str, shown: usize, wanted: usize, settled: bool) -> usize {
    let rest = &text[wanted..];
    if rest.starts_with(char::is_whitespace) {
        return wanted;
    }
    let mut chars = rest.char_indices();
    if let Some((index, _)) = chars
        .by_ref()
        .take(MAX_WORD)
        .find(|(_, c)| c.is_whitespace())
    {
        return wanted + index;
    }
    if chars.next().is_some() || settled {
        // A long run shows by characters.
        return if settled && rest.chars().count() <= MAX_WORD {
            text.len()
        } else {
            wanted
        };
    }
    // The last word can still become longer.
    match text[shown..wanted].rfind(char::is_whitespace) {
        Some(index) => shown + index,
        None if text[shown..wanted].chars().count() > MAX_WORD => wanted,
        None => shown,
    }
}

/// An inline span: code, bold text, or a link.
#[derive(Default)]
struct Spans {
    code: Option<(usize, usize)>,
    bold: Option<usize>,
    link: Option<(usize, bool)>,
}

impl Spans {
    fn open(&self) -> Option<usize> {
        [
            self.code.map(|(start, _)| start),
            self.bold,
            self.link.map(|(start, _)| start),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Reads the Markdown at `index` and returns the length that it used.
    fn read(&mut self, bytes: &[u8], index: usize) -> usize {
        let run = |byte| bytes[index..].iter().take_while(|&&b| b == byte).count();
        match bytes[index] {
            b'`' => {
                let length = run(b'`');
                match self.code {
                    Some((_, open)) if open == length => self.code = None,
                    Some(_) => {}
                    None => self.code = Some((index, length)),
                }
                length
            }
            _ if self.code.is_some() => 1,
            // An escaped ASCII character is text.
            b'\\' if bytes.get(index + 1).is_some_and(u8::is_ascii) => 2,
            b'*' if bytes.get(index + 1) == Some(&b'*') => {
                self.bold = match self.bold {
                    Some(_) => None,
                    None => Some(index),
                };
                2
            }
            b'[' if self.link.is_none() => {
                self.link = Some((index, false));
                1
            }
            b']' if matches!(self.link, Some((_, false))) => {
                if bytes.get(index + 1) == Some(&b'(') {
                    self.link = self.link.map(|(start, _)| (start, true));
                    2
                } else {
                    self.link = None;
                    1
                }
            }
            b')' if matches!(self.link, Some((_, true))) => {
                self.link = None;
                1
            }
            _ => 1,
        }
    }
}

/// Whether the line that starts at `start` opens or closes a code block.
fn fence(text: &str, start: usize) -> bool {
    let line = text[start..].trim_start_matches([' ', '\t']);
    line.starts_with("```") || line.starts_with("~~~")
}

/// Moves `cut` out of an inline span that is open at `cut`: to the end of
/// the span when the span ends on its line, or to its start when its line
/// is not complete. The text in a code block shows without this check.
fn span_end(text: &str, shown: usize, cut: usize, settled: bool) -> usize {
    let line = text[..cut].rfind('\n').map_or(0, |index| index + 1);
    let mut in_block = false;
    let mut start = 0;
    while start < line {
        in_block ^= fence(text, start);
        start += text[start..]
            .find('\n')
            .map_or(text.len(), |index| index + 1);
    }
    if in_block || fence(text, line) {
        return cut;
    }
    let bytes = text.as_bytes();
    let mut spans = Spans::default();
    let mut index = line;
    while index < cut {
        index += spans.read(bytes, index);
    }
    let Some(open) = spans.open() else {
        return cut;
    };
    // `index` can be after `cut` when `cut` splits a marker.
    while index < bytes.len() && bytes[index] != b'\n' {
        index += spans.read(bytes, index);
        if spans.open().is_none() {
            return index.min(text.len());
        }
    }
    if index < bytes.len() || settled {
        // The line ended, so the markers are text.
        cut
    } else {
        open.max(shown)
    }
}

fn table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// Moves `cut` out of a table row: to the end of the row when the row is
/// complete, or to its start. The header row shows with the delimiter row
/// below it.
fn row_end(text: &str, shown: usize, cut: usize, settled: bool) -> usize {
    let start = text[..cut].rfind('\n').map_or(0, |index| index + 1);
    if cut == start || !table_row(&text[start..]) {
        return cut;
    }
    let header =
        start == 0 || !table_row(&text[text[..start - 1].rfind('\n').map_or(0, |i| i + 1)..]);
    let line_end = |from: usize| text[from..].find('\n').map(|index| from + index);
    let end = line_end(cut).and_then(|end| if header { line_end(end + 1) } else { Some(end) });
    match end {
        Some(end) => end,
        None if settled => text.len(),
        None => start.max(shown),
    }
}

impl Qrow {
    /// Starts the steps that show streamed replies, unless they run.
    pub(super) fn reveal_assistant_replies(&mut self, cx: &mut Context<Self>) {
        if self.assistant_state.revealing {
            return;
        }
        self.assistant_state.revealing = true;
        cx.spawn(async move |this, cx| {
            let mut last = cx.background_executor().now();
            loop {
                cx.background_executor().timer(FRAME).await;
                let now = cx.background_executor().now();
                let elapsed = now - last;
                last = now;
                let more = this
                    .update(cx, |this, cx| {
                        let more = this.advance_assistant_replies(elapsed, cx);
                        this.assistant_state.revealing = more;
                        more
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// Shows more of the streamed replies of the shown conversation. Other
    /// conversations show their replies at once. Returns whether a reply
    /// still has text to show.
    fn advance_assistant_replies(&mut self, elapsed: Duration, cx: &mut Context<Self>) -> bool {
        let displayed = self.displayed_thread();
        let paced = !cx.reduce_motion();
        let mut changed = false;
        let mut more = false;
        for (thread, entries) in &mut self.assistant_state.transcripts {
            let shown = paced && displayed.as_deref() == Some(thread.as_str());
            for entry in entries.iter_mut().filter(|entry| entry.revealing()) {
                if shown {
                    changed |= entry.advance_reveal(elapsed);
                    more |= entry.revealing();
                } else {
                    changed |= entry.show_all();
                }
            }
        }
        if changed {
            self.sync_assistant_pane(cx);
        }
        more
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the reveal over `parts`, which arrive at their times in seconds,
    /// and returns the shown text after each frame.
    fn play(parts: &[(f32, &str)], seconds: f32) -> Vec<String> {
        let mut text = String::new();
        let mut reveal = Reveal::default();
        let mut frames = Vec::new();
        let mut next = 0;
        let mut time = 0.;
        while time < seconds {
            while next < parts.len() && parts[next].0 <= time {
                text.push_str(parts[next].1);
                reveal.received();
                next += 1;
            }
            reveal.advance(&text, FRAME);
            frames.push(text[..reveal.shown()].to_owned());
            time += FRAME.as_secs_f32();
        }
        frames
    }

    #[::core::prelude::v1::test]
    fn a_burst_shows_over_several_frames() {
        let part = "word ".repeat(16);
        let frames = play(&[(0., &part)], 2.);
        let first = frames.iter().position(|frame| !frame.is_empty()).unwrap();
        let last = frames
            .iter()
            .position(|frame| frame.len() >= part.trim_end().len())
            .unwrap();
        assert!(
            last - first >= 10,
            "the burst showed in {} frames",
            last - first
        );
        assert!(last < 60, "the burst took {last} frames");
    }

    #[::core::prelude::v1::test]
    fn irregular_parts_show_at_an_even_speed() {
        // 80 characters every 300 ms, like a reply in the video of the issue.
        let part = "abcd efgh ".repeat(8);
        let parts: Vec<_> = (0..10)
            .map(|index| (index as f32 * 0.3, part.as_str()))
            .collect();
        let frames = play(&parts, 3.);
        // After the start, the speed changes little from one 100 ms to the next.
        let lengths: Vec<_> = frames.iter().step_by(6).map(String::len).collect();
        let steps: Vec<_> = lengths
            .windows(2)
            .skip(3)
            .map(|pair| pair[1] - pair[0])
            .collect();
        let (min, max) = (steps.iter().min().unwrap(), steps.iter().max().unwrap());
        assert!(*min > 0, "the reveal stopped: {steps:?}");
        assert!(max - min <= 20, "the speed is not even: {steps:?}");
    }

    #[::core::prelude::v1::test]
    fn the_reveal_does_not_split_a_word_until_the_reply_pauses() {
        let frames = play(&[(0., "Hello wor")], 0.2);
        assert!(
            frames
                .iter()
                .all(|frame| frame.is_empty() || frame == "Hello")
        );
        assert_eq!(frames.last().unwrap(), "Hello");
        let frames = play(&[(0., "Hello wor")], 0.5);
        assert_eq!(frames.last().unwrap(), "Hello wor");
    }

    #[::core::prelude::v1::test]
    fn the_reveal_does_not_split_markdown_spans() {
        let text = "One **bold phrase here** and `some code` and [a link](https://example.com) end";
        for wanted in (1..text.len()).filter(|index| text.is_char_boundary(*index)) {
            let shown = &text[..cut(text, 0, wanted, false)];
            assert_eq!(shown.matches("**").count() % 2, 0, "{shown:?}");
            assert_eq!(shown.matches('`').count() % 2, 0, "{shown:?}");
            assert_eq!(
                shown.matches('[').count(),
                shown.matches(')').count(),
                "{shown:?}"
            );
        }
        // An open span waits for its end, and shows when the line ends without one.
        assert_eq!(cut("A **bold", 0, 6, false), 1);
        assert_eq!(cut("A **bold\nNext", 0, 6, false), 8);
        assert_eq!(cut("A **bold", 0, 6, true), 8);
    }

    #[::core::prelude::v1::test]
    fn markers_in_a_code_block_are_text() {
        assert_eq!(cut("```sql\nSELECT `a FROM t", 0, 15, false), 16);
        assert_eq!(cut("SELECT `a FROM t", 0, 8, false), 7);
    }

    #[::core::prelude::v1::test]
    fn table_rows_show_whole_and_the_header_shows_with_its_delimiter() {
        let text = "Rows:\n| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 |";
        let header = text.find("| a").unwrap();
        let delimiter_end = text.find("\n| 1").unwrap();
        let row_end = text.find("\n| 3").unwrap();
        assert_eq!(cut(text, 0, header + 3, false), delimiter_end);
        assert_eq!(cut(text, 0, delimiter_end + 4, false), row_end);
        assert_eq!(cut(text, 0, row_end + 4, false), row_end + 1);
        assert_eq!(cut(text, 0, row_end + 4, true), text.len());
    }

    #[::core::prelude::v1::test]
    fn long_runs_without_spaces_show_by_characters() {
        let text = "数据".repeat(40);
        assert_eq!(cut(&text, 0, 6, false), 6);
    }
}
