//! Turning Whisper's short segments into something to read and to save:
//! paragraphs joined at sentence ends and pauses, subtitle cues, plain text,
//! search, and the small time and size formats the window shows.

use std::path::{Path, PathBuf};

/// One stretch of speech as Whisper heard it. `text` keeps Whisper's own
/// leading space, so languages written without spaces join correctly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl Segment {
    pub fn new(start_ms: u64, end_ms: u64, text: &str) -> Self {
        Self {
            start_ms,
            end_ms,
            text: text.to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paragraph {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Whether Whisper's text is only a sound label such as `[BLANK_AUDIO]` or
/// `(music)`, which is not speech.
pub fn is_noise(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    let bracketed = |open: char, close: char| t.starts_with(open) && t.ends_with(close) && t[1..].find(close) == Some(t.len() - 2);
    bracketed('[', ']') || bracketed('(', ')') || t.chars().all(|c| matches!(c, '.' | '-' | ' ' | '*' | '♪'))
}

fn ends_sentence(text: &str) -> bool {
    let t = text.trim_end().trim_end_matches(['"', '\'', '”', '’', ')', '»']);
    t.ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

/// A pause at least this long always starts a new paragraph.
const LONG_PAUSE_MS: u64 = 2_500;
/// After a full stop, a pause this long starts a new paragraph.
const SENTENCE_PAUSE_MS: u64 = 1_000;
/// After a full stop, a paragraph this long (in characters) ends.
const SOFT_LENGTH: usize = 450;
/// With no full stop in sight, a paragraph this long ends anyway.
const HARD_LENGTH: usize = 1_200;

/// Joins segments into readable paragraphs: a new one starts at a long
/// pause, or at a sentence end after a short pause or once the paragraph is
/// long enough.
pub fn paragraphs(segments: &[Segment]) -> Vec<Paragraph> {
    let mut out: Vec<Paragraph> = Vec::new();
    let mut previous: Option<&Segment> = None;
    for segment in segments.iter().filter(|s| !is_noise(&s.text)) {
        let start_new = match (out.last(), previous) {
            (Some(current), Some(prev)) => {
                let gap = segment.start_ms.saturating_sub(prev.end_ms);
                let length = current.text.chars().count();
                let sentence = ends_sentence(&prev.text);
                gap >= LONG_PAUSE_MS
                    || (sentence && (gap >= SENTENCE_PAUSE_MS || length >= SOFT_LENGTH))
                    || length >= HARD_LENGTH
            }
            _ => true,
        };
        if start_new {
            out.push(Paragraph {
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                text: segment.text.trim_start().to_owned(),
            });
        } else if let Some(current) = out.last_mut() {
            current.text.push_str(&segment.text);
            current.end_ms = segment.end_ms;
        }
        previous = Some(segment);
    }
    for p in &mut out {
        p.text = p.text.trim().to_owned();
    }
    out
}

/// The transcript as plain text, paragraphs separated by blank lines.
pub fn plain_text(paragraphs: &[Paragraph]) -> String {
    paragraphs
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The TXT export: each paragraph under its start time.
pub fn to_txt(paragraphs: &[Paragraph]) -> String {
    let mut out = String::new();
    for p in paragraphs {
        out.push_str(&format!("[{}]\n{}\n\n", clock_full(p.start_ms), p.text));
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// Hours, minutes, seconds and milliseconds.
fn clock_parts(ms: u64) -> (u64, u64, u64, u64) {
    (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// `00:01:02` - hours always shown, for files.
fn clock_full(ms: u64) -> String {
    let (h, m, s, _) = clock_parts(ms);
    format!("{h:02}:{m:02}:{s:02}")
}

/// `00:01:02,345` for SRT, `00:01:02.345` for VTT.
pub fn cue_time(ms: u64, decimal: char) -> String {
    let (h, m, s, frac) = clock_parts(ms);
    format!("{h:02}:{m:02}:{s:02}{decimal}{frac:03}")
}

/// `1:02` or `1:02:03`, as shown beside each paragraph.
pub fn clock(ms: u64) -> String {
    let (h, m, s, _) = clock_parts(ms);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Longest subtitle line, in characters.
const LINE_CHARS: usize = 42;
/// A cue holds at most two lines.
const CUE_CHARS: usize = LINE_CHARS * 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    /// One or two lines.
    pub lines: Vec<String>,
}

/// Breaks `text` into pieces of at most `limit` characters at spaces (or
/// anywhere, for text written without spaces).
fn split_words(text: &str, limit: usize) -> Vec<String> {
    let text = text.trim();
    let mut pieces = Vec::new();
    let mut current = String::new();
    let spaced = text.contains(' ');
    let units: Vec<String> = if spaced {
        text.split_whitespace().map(str::to_owned).collect()
    } else {
        text.chars().map(String::from).collect()
    };
    for unit in units {
        let joiner = if spaced && !current.is_empty() { 1 } else { 0 };
        if !current.is_empty() && current.chars().count() + joiner + unit.chars().count() > limit {
            pieces.push(std::mem::take(&mut current));
        }
        if spaced && !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&unit);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Subtitle cues: each segment split into cues of up to two short lines,
/// timed in proportion to their length.
pub fn cues(segments: &[Segment]) -> Vec<Cue> {
    let mut out = Vec::new();
    for segment in segments.iter().filter(|s| !is_noise(&s.text)) {
        let chunks = split_words(&segment.text, CUE_CHARS);
        let total: usize = chunks.iter().map(|c| c.chars().count()).sum::<usize>().max(1);
        let span = segment.end_ms.saturating_sub(segment.start_ms);
        let mut at = segment.start_ms;
        let mut used = 0usize;
        for chunk in chunks {
            used += chunk.chars().count();
            let end = segment.start_ms + span * used as u64 / total as u64;
            let lines = split_words(&chunk, LINE_CHARS);
            out.push(Cue {
                start_ms: at,
                end_ms: end.max(at + 1),
                lines,
            });
            at = end;
        }
    }
    out
}

pub fn to_srt(segments: &[Segment]) -> String {
    let mut out = String::new();
    for (i, cue) in cues(segments).iter().enumerate() {
        out.push_str(&format!(
            "{}\r\n{} --> {}\r\n{}\r\n\r\n",
            i + 1,
            cue_time(cue.start_ms, ','),
            cue_time(cue.end_ms, ','),
            cue.lines.join("\r\n")
        ));
    }
    out
}

pub fn to_vtt(segments: &[Segment]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues(segments) {
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            cue_time(cue.start_ms, '.'),
            cue_time(cue.end_ms, '.'),
            cue.lines.join("\n")
        ));
    }
    out
}

/// Byte ranges of every case-insensitive match of `query` in `text`.
pub fn find(text: &str, query: &str) -> Vec<(usize, usize)> {
    let query: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut q = 0;
        let mut j = i;
        while q < query.len() && j < chars.len() {
            let lower: Vec<char> = chars[j].1.to_lowercase().collect();
            if query.get(q..q + lower.len()) != Some(lower.as_slice()) {
                break;
            }
            q += lower.len();
            j += 1;
        }
        if q == query.len() {
            let end = chars.get(j).map_or(text.len(), |(b, _)| *b);
            out.push((chars[i].0, end));
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// `dir/stem.ext`, or `dir/stem (2).ext` and so on: the first name that
/// `exists` says is free, so nothing is ever overwritten.
pub fn unique_path(dir: &Path, stem: &str, ext: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !exists(&first) {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !exists(p))
        .expect("some name is free")
}

/// Seconds left, from how long it has taken so far. `None` until there is
/// enough to go on.
pub fn remaining(elapsed_secs: f64, fraction: f32) -> Option<f64> {
    if fraction < 0.02 || elapsed_secs < 5.0 {
        return None;
    }
    let fraction = fraction.min(1.0) as f64;
    Some(elapsed_secs * (1.0 - fraction) / fraction)
}

pub fn format_remaining(secs: f64) -> String {
    let minutes = (secs / 60.0).round() as u64;
    if secs < 50.0 {
        "Less than a minute left".to_owned()
    } else if minutes < 60 {
        format!("About {} min left", minutes.max(1))
    } else {
        format!("About {} h {} min left", minutes / 60, minutes % 60)
    }
}

/// A recording's length: `35 s`, `42 min`, `1 h 05 min`.
pub fn format_length(secs: f64) -> String {
    let total = secs.round() as u64;
    if total < 60 {
        format!("{total} s")
    } else if total < 3600 {
        format!("{} min", (total + 30) / 60)
    } else {
        let minutes = (total + 30) / 60;
        format!("{} h {:02} min", minutes / 60, minutes % 60)
    }
}

/// Megabytes as Windows counts them.
pub fn format_mb(bytes: u64) -> String {
    format!("{} MB", (bytes + 512 * 1024) / (1024 * 1024))
}

/// Media files Windows can usually read.
const MEDIA: [&str; 18] = [
    "mp3", "m4a", "wav", "flac", "aac", "wma", "ogg", "opus", "amr", "mp4", "m4v", "mov", "mkv",
    "webm", "avi", "wmv", "3gp", "mpeg",
];

/// The extensions the open dialog offers.
pub fn media_extensions() -> &'static [&'static str] {
    &MEDIA
}

pub fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MEDIA.contains(&e.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: u64, end: u64, text: &str) -> Segment {
        Segment::new(start, end, text)
    }

    #[test]
    fn sound_labels_are_not_speech() {
        assert!(is_noise("[BLANK_AUDIO]"));
        assert!(is_noise(" (upbeat music) "));
        assert!(is_noise(" ♪ ♪"));
        assert!(is_noise(""));
        assert!(!is_noise(" Hello there."));
        assert!(!is_noise("(laughs) That was good (really)."));
    }

    #[test]
    fn short_segments_join_into_one_paragraph() {
        let segments = [
            seg(0, 2_000, " So the first thing"),
            seg(2_100, 4_000, " we looked at was cost,"),
            seg(4_100, 6_000, " which went up."),
        ];
        let p = paragraphs(&segments);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text, "So the first thing we looked at was cost, which went up.");
        assert_eq!((p[0].start_ms, p[0].end_ms), (0, 6_000));
    }

    #[test]
    fn paragraphs_break_at_a_sentence_end_with_a_pause() {
        let segments = [
            seg(0, 2_000, " That's the plan."),
            // Short pause after a full stop: same paragraph.
            seg(2_300, 4_000, " Any questions?"),
            // Longer pause after a question mark: new paragraph.
            seg(5_200, 7_000, " Yes, one."),
        ];
        let p = paragraphs(&segments);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].text, "That's the plan. Any questions?");
        assert_eq!(p[1].start_ms, 5_200);
    }

    #[test]
    fn a_long_pause_breaks_even_mid_sentence_and_noise_is_dropped() {
        let segments = [
            seg(0, 1_000, " and then"),
            seg(1_000, 9_000, " [BLANK_AUDIO]"),
            seg(9_000, 10_000, " we left"),
        ];
        let p = paragraphs(&segments);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].text, "we left");
    }

    #[test]
    fn long_runs_break_at_the_next_sentence_end() {
        let sentence = " This sentence is about fifty characters long, ok.";
        let segments: Vec<Segment> = (0..30).map(|i| seg(i * 3_000, i * 3_000 + 2_900, sentence)).collect();
        let p = paragraphs(&segments);
        assert!(p.len() > 2);
        for para in &p {
            assert!(para.text.chars().count() <= SOFT_LENGTH + sentence.len());
            assert!(para.text.ends_with('.'));
        }
    }

    #[test]
    fn text_without_spaces_joins_without_spaces() {
        let segments = [seg(0, 1_000, "今日は"), seg(1_000, 2_000, "いい天気です。")];
        assert_eq!(paragraphs(&segments)[0].text, "今日はいい天気です。");
    }

    #[test]
    fn times_are_formatted_for_screen_and_subtitles() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(62_000), "1:02");
        assert_eq!(clock(3_723_000), "1:02:03");
        assert_eq!(cue_time(3_723_045, ','), "01:02:03,045");
        assert_eq!(cue_time(5_500, '.'), "00:00:05.500");
    }

    #[test]
    fn srt_and_vtt_have_the_right_shape() {
        let segments = [seg(0, 2_500, " Hello and welcome."), seg(2_500, 5_000, " [MUSIC]")];
        let srt = to_srt(&segments);
        assert_eq!(srt, "1\r\n00:00:00,000 --> 00:00:02,500\r\nHello and welcome.\r\n\r\n");
        let vtt = to_vtt(&segments);
        assert!(vtt.starts_with("WEBVTT\n\n00:00:00.000 --> 00:00:02.500\nHello and welcome.\n"));
    }

    #[test]
    fn long_segments_become_several_short_cues() {
        let text = " The committee agreed that the new schedule would start in March, with a review after the first six weeks and a final report by the end of June.";
        let segments = [seg(10_000, 20_000, text)];
        let cues = cues(&segments);
        assert!(cues.len() >= 2);
        assert_eq!(cues[0].start_ms, 10_000);
        assert_eq!(cues.last().unwrap().end_ms, 20_000);
        for pair in cues.windows(2) {
            assert_eq!(pair[0].end_ms, pair[1].start_ms);
        }
        for cue in &cues {
            assert!(cue.lines.len() <= 2, "{cue:?}");
            assert!(cue.lines.iter().all(|l| l.chars().count() <= LINE_CHARS), "{cue:?}");
        }
        let rejoined: Vec<String> = cues.iter().flat_map(|c| c.lines.clone()).collect();
        assert_eq!(rejoined.join(" "), text.trim());
    }

    #[test]
    fn txt_puts_each_paragraph_under_its_time() {
        let p = [
            Paragraph { start_ms: 0, end_ms: 1, text: "One.".into() },
            Paragraph { start_ms: 3_723_000, end_ms: 1, text: "Two.".into() },
        ];
        assert_eq!(to_txt(&p), "[00:00:00]\nOne.\n\n[01:02:03]\nTwo.\n");
        assert_eq!(plain_text(&p), "One.\n\nTwo.");
    }

    #[test]
    fn search_ignores_case_and_returns_byte_ranges() {
        let text = "Budget first. The BUDGET, and the budgetary review. Café café";
        let hits = find(text, "budget");
        assert_eq!(hits.len(), 3);
        assert_eq!(&text[hits[1].0..hits[1].1], "BUDGET");
        let cafe = find(text, "CAFÉ");
        assert_eq!(cafe.len(), 2);
        assert_eq!(&text[cafe[0].0..cafe[0].1], "Café");
        assert!(find(text, "  ").is_empty());
        assert!(find(text, "zebra").is_empty());
        assert_eq!(find("aaa", "aa").len(), 1);
    }

    #[test]
    fn unique_path_never_reuses_a_taken_name() {
        let dir = Path::new("C:/out");
        let taken = [dir.join("Talk.srt"), dir.join("Talk (2).srt")];
        let path = unique_path(dir, "Talk", "srt", |p| taken.iter().any(|t| t == p));
        assert_eq!(path, dir.join("Talk (3).srt"));
        assert_eq!(unique_path(dir, "Talk", "vtt", |_| false), dir.join("Talk.vtt"));
    }

    #[test]
    fn time_left_waits_for_enough_progress() {
        assert_eq!(remaining(60.0, 0.01), None);
        assert_eq!(remaining(2.0, 0.5), None);
        assert_eq!(remaining(100.0, 0.25), Some(300.0));
        assert_eq!(format_remaining(20.0), "Less than a minute left");
        assert_eq!(format_remaining(300.0), "About 5 min left");
        assert_eq!(format_remaining(4_980.0), "About 1 h 23 min left");
    }

    #[test]
    fn lengths_and_sizes_read_naturally() {
        assert_eq!(format_length(35.2), "35 s");
        assert_eq!(format_length(42.0 * 60.0), "42 min");
        assert_eq!(format_length(3_900.0), "1 h 05 min");
        assert_eq!(format_mb(181 * 1024 * 1024), "181 MB");
    }

    #[test]
    fn media_files_are_recognised_by_extension() {
        assert!(is_media(Path::new("a/Interview.M4A")));
        assert!(is_media(Path::new("clip.webm")));
        assert!(!is_media(Path::new("notes.docx")));
        assert!(!is_media(Path::new("README")));
    }
}
