use crate::DEFAULT_DATE_FORMAT;
use chrono::{DateTime, FixedOffset, NaiveDateTime};
use colored::Color::*;
use colored::{Color, ColoredString, Colorize};
use regex::{Captures, Regex};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{fs, io};

pub fn process_log_files(log_files: Vec<LogFile>) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    ignore_broken_pipe(write_log_files(&mut out, &log_files))
}

fn write_log_files<W: Write>(out: &mut W, log_files: &[LogFile]) -> io::Result<()> {
    let color_index = ColorIndex::build(log_files);
    for (index, entry) in Merger::new(log_files) {
        let log_file = &log_files[index];
        for line in render_entry(entry, log_file, &color_index) {
            writeln!(out, "{line}")?;
        }
    }
    out.flush()
}

/// A downstream closing the pipe (`| head`) is a normal way to stop reading, not a failure.
/// It can surface on any write, including the final flush of the buffered output.
fn ignore_broken_pipe(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

/// Merges the entries of several log files by ascending timestamp.
///
/// Each file is sorted by `parse_entries` beforehand; ties are broken by file order
/// so that the output is stable across runs.
struct Merger<'a> {
    log_files: &'a [LogFile],
    offsets: Vec<usize>,
    staged: Vec<Option<&'a LogEntry>>,
}

impl<'a> Merger<'a> {
    fn new(log_files: &'a [LogFile]) -> Self {
        Merger {
            log_files,
            offsets: vec![0; log_files.len()],
            staged: vec![None; log_files.len()],
        }
    }
}

impl<'a> Iterator for Merger<'a> {
    type Item = (usize, &'a LogEntry);

    fn next(&mut self) -> Option<Self::Item> {
        // rehydrate empty staged lines
        let log_files = self.log_files;
        let offsets = &self.offsets;
        for (index, staged) in self.staged.iter_mut().enumerate() {
            if staged.is_none() {
                *staged = log_files[index].lines.get(offsets[index]);
            }
        }
        // find the earliest staged entry
        let mut min: Option<(usize, DateTime<FixedOffset>)> = None;
        for (index, staged) in self.staged.iter().enumerate() {
            if let Some(entry) = staged
                && min.is_none_or(|(_, min_ts)| entry.timestamp < min_ts)
            {
                min = Some((index, entry.timestamp));
            }
        }
        // nothing staged means all files are exhausted
        let (index, _) = min?;
        self.offsets[index] += 1;
        self.staged[index].take().map(|entry| (index, entry))
    }
}

/// Colors assigned to the ids found across all files.
struct ColorIndex {
    /// matches any known id on a word boundary, `None` when there is no id to color
    regex: Option<Regex>,
    colors: HashMap<String, Color>,
}

impl ColorIndex {
    fn build(log_files: &[LogFile]) -> ColorIndex {
        let mut colors = HashMap::new();
        for log_file in log_files {
            if let Some(id) = &log_file.id {
                colors.insert(id.clone(), log_file.color);
            }
        }
        // longest ids first so that `12` wins over `1` in the alternation
        let mut ids: Vec<&str> = colors.keys().map(String::as_str).collect();
        ids.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        let regex = if ids.is_empty() {
            None
        } else {
            let pattern = ids
                .iter()
                .map(|id| bounded_pattern(id))
                .collect::<Vec<_>>()
                .join("|");
            Regex::new(&pattern).ok()
        };
        ColorIndex { regex, colors }
    }
}

/// Escapes `id` and anchors it on word boundaries, but only on the sides where a
/// boundary can actually match (`\b` never matches next to a non-word character).
fn bounded_pattern(id: &str) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = id.chars().next().is_some_and(is_word);
    let end = id.chars().next_back().is_some_and(is_word);
    format!(
        "{}{}{}",
        if start { r"\b" } else { "" },
        regex::escape(id),
        if end { r"\b" } else { "" }
    )
}

fn level_regex() -> &'static Regex {
    static LEVEL_REGEX: OnceLock<Regex> = OnceLock::new();
    LEVEL_REGEX.get_or_init(|| Regex::new(r"\b(?:ERROR|WARN|INFO)\b").expect("valid level regex"))
}

/// Renders an entry and its continuation lines, one output line each.
fn render_entry(entry: &LogEntry, log_file: &LogFile, color_index: &ColorIndex) -> Vec<String> {
    let prefix = match &log_file.id_colored {
        Some(id) => format!("[{}][{}]", log_file.file_name_colored, id),
        None => format!("[{}]", log_file.file_name_colored),
    };
    let mut rendered = Vec::with_capacity(entry.leading.len() + 1 + entry.continuations.len());
    for leading in &entry.leading {
        rendered.push(format!("{prefix}{}", render_line(leading, 0, color_index)));
    }
    rendered.push(format!(
        "{prefix}{}",
        render_line(&entry.line, entry.message_start, color_index)
    ));
    for continuation in &entry.continuations {
        rendered.push(format!(
            "{prefix}{}",
            render_line(continuation, 0, color_index)
        ));
    }
    rendered
}

/// Colors the message part of a line, leaving the timestamp prefix untouched.
///
/// Ids are substituted first, on the raw text: doing it after the log level has
/// been colored would let a numeric id match the digits of an ANSI escape.
fn render_line(line: &str, message_start: usize, color_index: &ColorIndex) -> String {
    let (timestamp, message) = line.split_at(message_start);
    let message = match &color_index.regex {
        None => message.to_string(),
        Some(regex) => regex
            .replace_all(message, |captures: &Captures| {
                let id = &captures[0];
                match color_index.colors.get(id) {
                    Some(color) => format!("{}", id.color(*color)),
                    None => id.to_string(),
                }
            })
            .into_owned(),
    };
    let message = level_regex().replace_all(&message, |captures: &Captures| {
        let level = &captures[0];
        match level {
            "ERROR" => format!("{}", level.color(Red).bold()),
            "WARN" => format!("{}", level.color(Yellow).bold()),
            _ => format!("{}", level.bold()),
        }
    });
    format!("{timestamp}{message}")
}

/// Parses the timestamp at the beginning of `line`.
///
/// Returns the timestamp and the byte offset at which the rest of the line starts.
fn parse_date(line: &str, date_format: &str) -> Option<(DateTime<FixedOffset>, usize)> {
    if date_format == DEFAULT_DATE_FORMAT {
        // `%Z` cannot be parsed back into an offset by chrono, but the default format is
        // RFC3339 - and an RFC3339 timestamp never contains whitespace.
        let token = line.split_whitespace().next()?;
        if !line.starts_with(token) {
            return None;
        }
        let timestamp = DateTime::parse_from_rfc3339(token).ok()?;
        return Some((timestamp, token.len()));
    }
    // formats carrying an offset (`%z`, `%:z`, ...)
    if let Ok((timestamp, rest)) = DateTime::parse_and_remainder(line, date_format) {
        return Some((timestamp, line.len() - rest.len()));
    }
    // formats without an offset: assume UTC so that files stay comparable
    let (naive, rest) = NaiveDateTime::parse_and_remainder(line, date_format).ok()?;
    Some((naive.and_utc().fixed_offset(), line.len() - rest.len()))
}

#[derive(Debug)]
pub struct LogEntry {
    timestamp: DateTime<FixedOffset>,
    line: String,
    /// byte offset of the message, right after the timestamp
    message_start: usize,
    /// following lines without a timestamp of their own (stack traces, wrapped messages)
    continuations: Vec<String>,
    /// lines that preceded the first timestamp of the file (banners, startup crashes),
    /// carried by the earliest entry so that they open the file's part of the story
    leading: Vec<String>,
}

pub struct LogFile {
    lines: Vec<LogEntry>,
    id: Option<String>,
    color: Color,
    // cached values
    file_name_colored: ColoredString,
    id_colored: Option<ColoredString>,
}

// Keep RED & YELLOW out of this to use it for log level
const COLORS_FOR_IDS: [Color; 10] = [
    Green,
    Blue,
    Magenta,
    Cyan,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
];

/// TODO Works with iterator instead of assuming files fit in memory \o/
/// Maybe merging with https://docs.rs/itertools/latest/itertools/structs/type.KMerge.html
pub fn load_files_in_memory(
    input_dir: &str,
    id_detection_regex: Option<&Regex>,
    date_format: &str,
) -> io::Result<Vec<LogFile>> {
    // validate input dir
    let logs_path = Path::new(input_dir);
    if !logs_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{input_dir} does not exist"),
        ));
    }
    if !logs_path.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("{input_dir} is not a directory"),
        ));
    }
    // keep only '.log' paths, sorted to make the output stable across runs
    let mut log_paths = fs::read_dir(logs_path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<PathBuf>>>()?;
    log_paths.retain(|path| path.extension().and_then(|os| os.to_str()) == Some("log"));
    log_paths.sort();
    if log_paths.is_empty() {
        eprintln!("WARN: no '.log' file found in {input_dir}");
    }
    // result
    let mut log_files = Vec::with_capacity(log_paths.len());
    // colors attribution
    let mut colors_mapping: HashMap<String, Color> = HashMap::new();
    // analyze paths
    for path in log_paths {
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let content: Vec<String> = reader.lines().collect::<io::Result<_>>()?;
        // extract id
        let id = id_detection_regex.and_then(|regex| match extract_id(&content, regex) {
            None => {
                eprintln!("No id found for file {file_name}");
                None
            }
            Some(id) => {
                eprintln!("Found id {id} for file {file_name}");
                Some(id)
            }
        });
        let lines = parse_entries(content, date_format, &file_name);
        // attribute color by id or file name, cycling when there are more keys than colors
        let key_color = id.clone().unwrap_or_else(|| file_name.clone());
        let next_color = colors_mapping.len();
        let color = *colors_mapping
            .entry(key_color)
            .or_insert_with(|| COLORS_FOR_IDS[next_color % COLORS_FOR_IDS.len()]);
        let file_name_colored = file_name.color(color);
        let id_colored = id.as_ref().map(|it| it.color(color));
        let log_file = LogFile {
            id,
            lines,
            color,
            file_name_colored,
            id_colored,
        };
        log_files.push(log_file);
    }
    Ok(log_files)
}

/// Turns raw lines into entries, attaching lines without a timestamp to the entry above them.
fn parse_entries(content: Vec<String>, date_format: &str, file_name: &str) -> Vec<LogEntry> {
    let mut entries: Vec<LogEntry> = Vec::with_capacity(content.len());
    let mut leading: Vec<String> = Vec::new();
    for line in content {
        // blank lines carry no information once files are interleaved
        if line.trim().is_empty() {
            continue;
        }
        match parse_date(&line, date_format) {
            Some((timestamp, message_start)) => entries.push(LogEntry {
                timestamp,
                line,
                message_start,
                continuations: Vec::new(),
                leading: Vec::new(),
            }),
            // no timestamp: continuation of the previous entry, or - when nothing has been
            // stamped yet - part of whatever the file printed before it started logging
            None => match entries.last_mut() {
                Some(previous) => previous.continuations.push(line),
                None => leading.push(line),
            },
        }
    }
    // a single file is not necessarily sorted: threads race between stamping and writing,
    // and the merge below can only be as chronological as the files it is fed
    entries.sort_by_key(|entry| entry.timestamp);
    // the lines above the first timestamp happened before anything this file stamped, so
    // they ride with its earliest entry rather than being guessed a timestamp of their own
    match entries.first_mut() {
        Some(earliest) => earliest.leading = leading,
        // nothing was ever stamped: there is no point in the story to hang them from
        None if !leading.is_empty() => eprintln!(
            "WARN: {file_name}: dropped {} line(s), no timestamp found in the whole file",
            leading.len()
        ),
        None => (),
    }
    entries
}

fn extract_id(lines: &[String], id_regex: &Regex) -> Option<String> {
    for line in lines {
        match id_regex.captures(line).and_then(|c| c.get(1)) {
            None => continue,
            Some(matched) => {
                let id = matched.as_str().to_string();
                return Some(id);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn entry(line: &str) -> LogEntry {
        let (timestamp, message_start) =
            parse_date(line, DEFAULT_DATE_FORMAT).expect("valid default timestamp");
        LogEntry {
            timestamp,
            line: line.to_string(),
            message_start,
            continuations: Vec::new(),
            leading: Vec::new(),
        }
    }

    fn log_file(file_name: &str, id: Option<&str>, color: Color, lines: Vec<LogEntry>) -> LogFile {
        let id = id.map(str::to_string);
        LogFile {
            lines,
            color,
            file_name_colored: file_name.color(color),
            id_colored: id.as_ref().map(|it| it.color(color)),
            id,
        }
    }

    /// `colored`'s override is global, so color assertions must be serialized and must
    /// build their expected value while the override is still in place.
    fn with_color<T>(enabled: bool, body: impl FnOnce() -> T) -> T {
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        colored::control::set_override(enabled);
        let result = body();
        colored::control::unset_override();
        result
    }

    #[test]
    fn parses_the_default_rfc3339_format() {
        let line = "2023-01-10T10:00:01.000000+00:00 INFO hello";
        let (timestamp, message_start) = parse_date(line, DEFAULT_DATE_FORMAT).unwrap();
        assert_eq!(timestamp.timestamp(), 1673344801);
        assert_eq!(&line[message_start..], " INFO hello");
    }

    #[test]
    fn rejects_a_line_without_a_leading_timestamp() {
        assert!(parse_date("   at com.foo.Bar(Bar.java:42)", DEFAULT_DATE_FORMAT).is_none());
        assert!(parse_date("", DEFAULT_DATE_FORMAT).is_none());
        // a timestamp not at the very beginning of the line is not a timestamp
        assert!(parse_date(" 2023-01-10T10:00:01.000000+00:00 x", DEFAULT_DATE_FORMAT).is_none());
    }

    #[test]
    fn parses_a_custom_format_containing_a_space() {
        let line = "2023-01-10 10:00:01,123 INFO hello";
        let (timestamp, message_start) = parse_date(line, "%Y-%m-%d %H:%M:%S,%3f").unwrap();
        assert_eq!(timestamp.timestamp(), 1673344801);
        assert_eq!(&line[message_start..], " INFO hello");
    }

    #[test]
    fn parses_a_custom_format_carrying_an_offset() {
        let line = "2023-01-10 10:00:01 +0200 INFO hello";
        let (timestamp, message_start) = parse_date(line, "%Y-%m-%d %H:%M:%S %z").unwrap();
        assert_eq!(timestamp.offset().local_minus_utc(), 2 * 3600);
        assert_eq!(&line[message_start..], " INFO hello");
    }

    #[test]
    fn parses_a_short_custom_format() {
        // shorter than the default format, which used to be rejected on length alone
        let line = "2023-01-10T10:00:01 INFO hi";
        let (timestamp, message_start) = parse_date(line, "%Y-%m-%dT%H:%M:%S").unwrap();
        assert_eq!(timestamp.timestamp(), 1673344801);
        assert_eq!(&line[message_start..], " INFO hi");
    }

    #[test]
    fn extracts_the_first_capture_group() {
        let regex = Regex::new(r"(?:newRaft, raft_id: )(\d+)").unwrap();
        let lines = vec![
            "no id here".to_string(),
            "2023-01-10T10:00:01.000000+00:00 newRaft, raft_id: 42 up".to_string(),
            "2023-01-10T10:00:02.000000+00:00 newRaft, raft_id: 7 up".to_string(),
        ];
        assert_eq!(extract_id(&lines, &regex), Some("42".to_string()));
        assert_eq!(extract_id(&[], &regex), None);
    }

    #[test]
    fn merges_files_by_timestamp() {
        let a = log_file(
            "a.log",
            None,
            Green,
            vec![
                entry("2023-01-10T10:00:01.000000+00:00 a1"),
                entry("2023-01-10T10:00:03.000000+00:00 a3"),
            ],
        );
        let b = log_file(
            "b.log",
            None,
            Blue,
            vec![
                entry("2023-01-10T10:00:02.000000+00:00 b2"),
                entry("2023-01-10T10:00:04.000000+00:00 b4"),
            ],
        );
        let files = vec![a, b];
        let merged: Vec<&str> = Merger::new(&files)
            .map(|(index, entry)| {
                let suffix = &entry.line[entry.message_start..];
                assert_eq!(index, if suffix.starts_with(" a") { 0 } else { 1 });
                suffix
            })
            .collect();
        assert_eq!(merged, vec![" a1", " b2", " a3", " b4"]);
    }

    #[test]
    fn breaks_timestamp_ties_by_file_order() {
        let stamp = "2023-01-10T10:00:01.000000+00:00";
        let files = vec![
            log_file("a.log", None, Green, vec![entry(&format!("{stamp} a"))]),
            log_file("b.log", None, Blue, vec![entry(&format!("{stamp} b"))]),
        ];
        let indexes: Vec<usize> = Merger::new(&files).map(|(index, _)| index).collect();
        assert_eq!(indexes, vec![0, 1]);
    }

    #[test]
    fn merges_empty_and_exhausted_files() {
        let files = vec![
            log_file("empty.log", None, Green, vec![]),
            log_file(
                "b.log",
                None,
                Blue,
                vec![entry("2023-01-10T10:00:01.000000+00:00 b")],
            ),
        ];
        assert_eq!(Merger::new(&files).count(), 1);
        assert_eq!(Merger::new(&[]).count(), 0);
    }

    #[test]
    fn attaches_lines_without_a_timestamp_to_the_previous_entry() {
        let content = vec![
            "banner without timestamp".to_string(),
            "2023-01-10T10:00:01.000000+00:00 boom".to_string(),
            "   at com.foo.Bar(Bar.java:42)".to_string(),
            "   at com.foo.Baz(Baz.java:7)".to_string(),
            "2023-01-10T10:00:02.000000+00:00 next".to_string(),
        ];
        let entries = parse_entries(content, DEFAULT_DATE_FORMAT, "a.log");
        assert_eq!(entries.len(), 2);
        // the banner is kept, carried by the earliest entry
        assert_eq!(
            entries[0].leading,
            vec!["banner without timestamp".to_string()]
        );
        assert_eq!(
            entries[0].continuations,
            vec![
                "   at com.foo.Bar(Bar.java:42)".to_string(),
                "   at com.foo.Baz(Baz.java:7)".to_string(),
            ]
        );
        assert!(entries[1].continuations.is_empty());
    }

    #[test]
    fn sorts_an_unsorted_file_keeping_continuations_with_their_entry() {
        // real logs interleave threads, so a file is not necessarily sorted
        let content = vec![
            "2023-01-10T10:00:03.000000+00:00 third".to_string(),
            "   trace of third".to_string(),
            "2023-01-10T10:00:01.000000+00:00 first".to_string(),
            "2023-01-10T10:00:02.000000+00:00 second".to_string(),
        ];
        let entries = parse_entries(content, DEFAULT_DATE_FORMAT, "a.log");
        let messages: Vec<&str> = entries
            .iter()
            .map(|entry| &entry.line[entry.message_start..])
            .collect();
        assert_eq!(messages, vec![" first", " second", " third"]);
        assert_eq!(
            entries[2].continuations,
            vec!["   trace of third".to_string()]
        );
        assert!(entries[0].continuations.is_empty());
    }

    #[test]
    fn sorting_a_file_is_stable_on_equal_timestamps() {
        let stamp = "2023-01-10T10:00:01.000000+00:00";
        let content = vec![
            format!("{stamp} b"),
            format!("{stamp} a"),
            format!("{stamp} c"),
        ];
        let entries = parse_entries(content, DEFAULT_DATE_FORMAT, "a.log");
        let messages: Vec<&str> = entries
            .iter()
            .map(|entry| &entry.line[entry.message_start..])
            .collect();
        assert_eq!(messages, vec![" b", " a", " c"]);
    }

    #[test]
    fn keeps_lines_preceding_the_first_timestamp() {
        // a startup crash is printed before anything gets logged - issue #3
        let content = vec![
            "thread 'main' panicked at src/lib.rs:1: STARTUP CRASH".to_string(),
            "note: run with RUST_BACKTRACE=1".to_string(),
            "2023-01-10T10:00:01.000000+00:00 started".to_string(),
            "2023-01-10T10:00:02.000000+00:00 next".to_string(),
        ];
        let entries = parse_entries(content, DEFAULT_DATE_FORMAT, "a.log");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].leading,
            vec![
                "thread 'main' panicked at src/lib.rs:1: STARTUP CRASH".to_string(),
                "note: run with RUST_BACKTRACE=1".to_string(),
            ]
        );
        assert!(entries[1].leading.is_empty());
    }

    #[test]
    fn leading_lines_ride_with_the_earliest_entry_not_the_first_written() {
        // the file is unsorted, so the first line written is not the earliest one
        let content = vec![
            "banner".to_string(),
            "2023-01-10T10:00:09.000000+00:00 written first".to_string(),
            "2023-01-10T10:00:01.000000+00:00 actually earliest".to_string(),
        ];
        let entries = parse_entries(content, DEFAULT_DATE_FORMAT, "a.log");
        let messages: Vec<&str> = entries
            .iter()
            .map(|entry| &entry.line[entry.message_start..])
            .collect();
        assert_eq!(messages, vec![" actually earliest", " written first"]);
        assert_eq!(entries[0].leading, vec!["banner".to_string()]);
        assert!(entries[1].leading.is_empty());
    }

    #[test]
    fn a_file_without_any_timestamp_yields_no_entry() {
        let content = vec!["banner".to_string(), "more banner".to_string()];
        assert!(parse_entries(content, DEFAULT_DATE_FORMAT, "a.log").is_empty());
    }

    #[test]
    fn renders_leading_lines_before_their_entry() {
        let content = vec![
            "STARTUP CRASH".to_string(),
            "2023-01-10T10:00:01.000000+00:00 started".to_string(),
            "   trailing detail".to_string(),
        ];
        let mut entries = parse_entries(content, DEFAULT_DATE_FORMAT, "a.log");
        let log_entry = entries.remove(0);
        let rendered = with_color(false, || {
            let file = log_file("a.log", None, Green, vec![]);
            render_entry(&log_entry, &file, &ColorIndex::build(&[]))
        });
        assert_eq!(
            rendered,
            vec![
                "[a.log]STARTUP CRASH".to_string(),
                "[a.log]2023-01-10T10:00:01.000000+00:00 started".to_string(),
                "[a.log]   trailing detail".to_string(),
            ]
        );
    }

    #[test]
    fn leading_lines_of_one_file_do_not_jump_ahead_of_another_file() {
        // they open their own file's part of the story, not the whole merge
        let a = log_file(
            "a.log",
            None,
            Green,
            parse_entries(
                vec![
                    "LATE FILE BANNER".to_string(),
                    "2023-01-10T10:00:05.000000+00:00 a5".to_string(),
                ],
                DEFAULT_DATE_FORMAT,
                "a.log",
            ),
        );
        let b = log_file(
            "b.log",
            None,
            Blue,
            parse_entries(
                vec!["2023-01-10T10:00:01.000000+00:00 b1".to_string()],
                DEFAULT_DATE_FORMAT,
                "b.log",
            ),
        );
        let files = vec![a, b];
        let color_index = ColorIndex::build(&files);
        let rendered: Vec<String> = with_color(false, || {
            Merger::new(&files)
                .flat_map(|(index, entry)| render_entry(entry, &files[index], &color_index))
                .collect()
        });
        assert_eq!(
            rendered,
            vec![
                "[b.log]2023-01-10T10:00:01.000000+00:00 b1".to_string(),
                "[a.log]LATE FILE BANNER".to_string(),
                "[a.log]2023-01-10T10:00:05.000000+00:00 a5".to_string(),
            ]
        );
    }

    #[test]
    fn drops_blank_lines_instead_of_attaching_them() {
        let content = vec![
            "".to_string(),
            "2023-01-10T10:00:01.000000+00:00 boom".to_string(),
            "".to_string(),
            "   ".to_string(),
            "\t".to_string(),
            "   at com.foo.Bar(Bar.java:42)".to_string(),
        ];
        let entries = parse_entries(content, DEFAULT_DATE_FORMAT, "a.log");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].continuations,
            vec!["   at com.foo.Bar(Bar.java:42)".to_string()]
        );
    }

    #[test]
    fn renders_an_entry_with_its_continuations() {
        let mut log_entry = entry("2023-01-10T10:00:01.000000+00:00 boom");
        log_entry
            .continuations
            .push("   at Bar.java:42".to_string());
        let rendered = with_color(false, || {
            let file = log_file("a.log", Some("1"), Green, vec![]);
            let color_index = ColorIndex::build(&[log_file("a.log", Some("1"), Green, vec![])]);
            render_entry(&log_entry, &file, &color_index)
        });
        assert_eq!(
            rendered,
            vec![
                "[a.log][1]2023-01-10T10:00:01.000000+00:00 boom".to_string(),
                "[a.log][1]   at Bar.java:42".to_string(),
            ]
        );
    }

    #[test]
    fn renders_a_file_without_id_with_a_single_prefix() {
        let log_entry = entry("2023-01-10T10:00:01.000000+00:00 hi");
        let rendered = with_color(false, || {
            let file = log_file("a.log", None, Green, vec![]);
            let color_index = ColorIndex::build(&[]);
            render_entry(&log_entry, &file, &color_index)
        });
        assert_eq!(rendered, vec!["[a.log]2023-01-10T10:00:01.000000+00:00 hi"]);
    }

    #[test]
    fn colors_ids_on_word_boundaries_only_and_never_the_timestamp() {
        with_color(true, || {
            let files = vec![log_file("a.log", Some("1"), Green, vec![])];
            let color_index = ColorIndex::build(&files);
            // `1` appears in the timestamp, inside `11`, and standalone
            let line = "2023-01-10T10:00:01.000000+00:00 peer 1 count=11 ok";
            let rendered = render_line(line, 32, &color_index);
            let expected = format!(
                "2023-01-10T10:00:01.000000+00:00 peer {} count=11 ok",
                "1".color(Green)
            );
            assert_eq!(rendered, expected);
        });
    }

    #[test]
    fn colors_log_levels_without_corrupting_escapes() {
        with_color(true, || {
            let files = vec![log_file("a.log", Some("1"), Green, vec![])];
            let color_index = ColorIndex::build(&files);
            let line = "2023-01-10T10:00:01.000000+00:00 INFO node 1 up";
            let rendered = render_line(line, 32, &color_index);
            let expected = format!(
                "2023-01-10T10:00:01.000000+00:00 {} node {} up",
                "INFO".bold(),
                "1".color(Green)
            );
            assert_eq!(rendered, expected);
            // the bold escape `\x1b[1m` must survive the id substitution untouched
            assert!(rendered.contains("\u{1b}[1mINFO"));
        });
    }

    #[test]
    fn does_not_color_a_level_embedded_in_a_word() {
        with_color(true, || {
            let color_index = ColorIndex::build(&[]);
            let rendered = render_line("x WARNING and INFORMATION", 1, &color_index);
            assert_eq!(rendered, "x WARNING and INFORMATION");
        });
    }

    #[test]
    fn prefers_the_longest_id_when_ids_overlap() {
        with_color(true, || {
            let files = vec![
                log_file("a.log", Some("1"), Green, vec![]),
                log_file("b.log", Some("12"), Blue, vec![]),
            ];
            let color_index = ColorIndex::build(&files);
            let rendered = render_line("x 12 1", 1, &color_index);
            let expected = format!(" {} {}", "12".color(Blue), "1".color(Green));
            assert_eq!(rendered, format!("x{expected}"));
        });
    }

    #[test]
    fn color_attribution_cycles_past_the_palette() {
        // more distinct keys than available colors used to panic on `Vec::remove(0)`
        let mut colors_mapping: HashMap<String, Color> = HashMap::new();
        let mut attributed = Vec::new();
        for i in 0..(COLORS_FOR_IDS.len() * 2 + 3) {
            let next_color = colors_mapping.len();
            let color = *colors_mapping
                .entry(format!("id-{i}"))
                .or_insert_with(|| COLORS_FOR_IDS[next_color % COLORS_FOR_IDS.len()]);
            attributed.push(color);
        }
        assert_eq!(attributed.len(), COLORS_FOR_IDS.len() * 2 + 3);
        assert_eq!(attributed[0], attributed[COLORS_FOR_IDS.len()]);
    }

    #[test]
    fn bounded_pattern_skips_boundaries_next_to_non_word_characters() {
        assert_eq!(bounded_pattern("42"), r"\b42\b");
        assert_eq!(bounded_pattern("+x"), r"\+x\b");
        assert!(
            Regex::new(&bounded_pattern("+x"))
                .unwrap()
                .is_match("a +x ")
        );
    }

    /// Fails every write, or only the flush, with the given error kind.
    struct FailingWriter {
        kind: io::ErrorKind,
        fail_writes: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.fail_writes {
                Err(io::Error::from(self.kind))
            } else {
                Ok(buf.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(self.kind))
        }
    }

    fn one_entry_file() -> Vec<LogFile> {
        vec![log_file(
            "a.log",
            None,
            Green,
            vec![entry("2023-01-10T10:00:01.000000+00:00 hi")],
        )]
    }

    #[test]
    fn a_broken_pipe_on_write_is_not_an_error() {
        let mut writer = FailingWriter {
            kind: io::ErrorKind::BrokenPipe,
            fail_writes: true,
        };
        let result = write_log_files(&mut writer, &one_entry_file());
        assert_eq!(
            result.as_ref().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(ignore_broken_pipe(result).is_ok());
    }

    #[test]
    fn a_broken_pipe_on_the_final_flush_is_not_an_error() {
        // output small enough to sit in the buffer: the pipe only breaks on flush
        assert!(ignore_broken_pipe(result_of_buffered(io::ErrorKind::BrokenPipe)).is_ok());
    }

    #[test]
    fn any_other_write_error_is_reported() {
        let error = ignore_broken_pipe(result_of_buffered(io::ErrorKind::StorageFull)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
    }

    fn result_of_buffered(kind: io::ErrorKind) -> io::Result<()> {
        let mut writer = BufWriter::new(FailingWriter {
            kind,
            fail_writes: false,
        });
        write_log_files(&mut writer, &one_entry_file())
    }

    #[test]
    fn missing_input_dir_is_reported_as_an_error() {
        match load_files_in_memory("/does/not/exist", None, DEFAULT_DATE_FORMAT) {
            Err(err) => assert_eq!(err.kind(), io::ErrorKind::NotFound),
            Ok(_) => panic!("expected a NotFound error"),
        }
    }
}
