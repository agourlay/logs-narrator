use chrono::{DateTime, FixedOffset};
use colored::Color::*;
use colored::Colorize;
use colored::{Color, ColoredString};
use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::{env, fs, io};

fn main() -> io::Result<()> {
    // TODO CLI args management
    let input_dir = env::args().nth(1).unwrap();
    // TODO inject id_detection from CLI
    let id_detection_regex = Regex::new(r"(?:newRaft, raft_id: )(\d+)").unwrap();

    let log_files = load_files_in_memory(&input_dir, Some(id_detection_regex))?;
    process_log_files(log_files);
    Ok(())
}

fn process_log_files(log_files: Vec<LogFile>) {
    // color index
    let mut color_index = HashMap::new();
    for log_file in &log_files {
        if let Some(log_file_id) = &log_file.id {
            color_index.insert(log_file_id.clone(), log_file.id_colored.as_ref().unwrap());
        }
    }
    // hold the current offset for each files
    let mut offsets: Vec<usize> = vec![0; log_files.len()];
    let mut staged_lines: Vec<Option<LogEntry>> = vec![None; log_files.len()];
    loop {
        // rehydrate empty staged lines
        for (index, offset) in offsets.iter().enumerate() {
            if staged_lines[index].is_none() {
                if let Some(line) = log_files[index].lines.get(*offset) {
                    staged_lines[index] = Some(line.clone());
                }
            }
        }
        // exit if nothing staged
        if staged_lines.iter().all(|l| l.is_none()) {
            break;
        }
        // find value and pop it
        let mut min_index: Option<(usize, LogEntry)> = None;
        for (index, staged_line) in staged_lines.iter().enumerate() {
            match staged_line {
                None => (),
                Some(sl) => {
                    match &min_index {
                        None => min_index = Some((index, sl.clone())), // first element
                        Some((_index, existing_min)) if existing_min.timestamp > sl.timestamp => {
                            min_index = Some((index, sl.clone())) // new min
                        }
                        _ => (),
                    }
                }
            }
        }
        match min_index {
            None => panic!("broken invariant - there must a min"),
            Some((min_index, sl)) => {
                // inc offset
                offsets[min_index] += 1;
                // remove stage value
                staged_lines[min_index].take();
                // print line
                let log_file = &log_files[min_index];
                render_log_entry(&sl, log_file, &color_index)
            }
        }
    }
}

fn render_log_entry(
    log_entry: &LogEntry,
    log_file: &LogFile,
    color_index: &HashMap<String, &ColoredString>,
) {
    let mut tmp_line = log_entry.line.clone();
    // naive color log level
    tmp_line = tmp_line.replace("ERROR", &format!("{}", "ERROR".color(Red).bold()));
    tmp_line = tmp_line.replace("WARN", &format!("{}", "WARN".color(Yellow).bold()));
    tmp_line = tmp_line.replace("INFO", &format!("{}", "INFO".bold()));

    if color_index.is_empty() {
        // no additional coloring
        println!("[{}]{}", log_file.file_name_colored, tmp_line);
    } else {
        let prefix = log_file
            .id_colored
            .as_ref()
            .unwrap_or(&log_file.file_name_colored);

        // color ids founds
        for (id, colored_id) in color_index {
            tmp_line = tmp_line.replace(id, &format!("{}", &colored_id));
        }

        println!("[{}][{}]{}", log_file.file_name, prefix, tmp_line);
    }
}

fn parse_date(line: &str) -> Option<DateTime<FixedOffset>> {
    if line.chars().count() > 25 {
        let date_str = &line[1..25]; // focus on date (will break on other format!)
        DateTime::parse_from_rfc3339(date_str).ok()
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialOrd, PartialEq)]
struct LogEntry {
    timestamp: DateTime<FixedOffset>,
    line: String,
}

struct LogFile {
    file_name: String,
    lines: Vec<LogEntry>,
    id: Option<String>,
    // cached values
    file_name_colored: ColoredString,
    id_colored: Option<ColoredString>,
}

// TODO Generate more than 10 colors
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

/// Assume files fit in memory \o/
fn load_files_in_memory(
    input_dir: &str,
    id_detection_regex: Option<Regex>,
) -> io::Result<Vec<LogFile>> {
    let paths = fs::read_dir(input_dir).unwrap();
    // result
    let mut log_files = Vec::new();
    // keep only '.log' paths
    let log_paths = paths.filter(|p| {
        let path = p.as_ref().unwrap().path();
        path.extension().and_then(|os| os.to_str()) == Some("log")
    });
    // analyze paths
    for (index, path) in log_paths.enumerate() {
        let path = path?.path();
        let file_name = path
            .file_name()
            .and_then(|os| os.to_str())
            .unwrap()
            .to_string();
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let content: io::Result<Vec<String>> = reader.lines().collect();
        let content = content?;
        // extract id
        let id = id_detection_regex
            .as_ref()
            .and_then(|regex| match extract_id(&content, regex) {
                None => {
                    println!("Regex provided but no capture data found");
                    None
                }
                Some(id) => {
                    println!("Found id {} for file {}", id, file_name);
                    Some(id)
                }
            });
        let lines = content
            .into_iter()
            .filter_map(|line| match parse_date(&line) {
                None => {
                    println!("WARN:Could not find valid timestamp in line:{}", &line);
                    None
                }
                Some(timestamp) => Some(LogEntry { timestamp, line }),
            })
            .collect();
        let color = COLORS_FOR_IDS[index];
        let file_name_colored = file_name.color(color);
        let id_colored = id.as_ref().map(|it| it.color(color));
        let log_file = LogFile {
            file_name,
            id,
            lines,
            file_name_colored,
            id_colored,
        };
        log_files.push(log_file);
    }
    Ok(log_files)
}

fn extract_id(lines: &Vec<String>, id_regex: &Regex) -> Option<String> {
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
