// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading log files a line at a time.

use crate::error::{ToolError, ToolResult};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Lines of a log file with invalid UTF-8 replaced and line endings removed,
/// read a line at a time so a large log is never held in memory. A read
/// error ends the iteration and is reported by [`LogLines::finish`].
pub struct LogLines {
    path: PathBuf,
    reader: BufReader<File>,
    raw: Vec<u8>,
    error: Option<std::io::Error>,
}

impl LogLines {
    pub fn open(path: &Path) -> ToolResult<Self> {
        let file = File::open(path).map_err(|error| ToolError::io("open", path, error))?;
        Ok(Self {
            path: path.to_path_buf(),
            reader: BufReader::with_capacity(256 * 1024, file),
            raw: Vec::with_capacity(512),
            error: None,
        })
    }

    /// The read error that ended the iteration early, if any.
    pub fn finish(self) -> ToolResult<()> {
        match self.error {
            None => Ok(()),
            Some(error) => Err(ToolError::io("read", &self.path, error)),
        }
    }
}

impl Iterator for LogLines {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        self.raw.clear();
        match self.reader.read_until(b'\n', &mut self.raw) {
            Ok(0) => None,
            Ok(_) => {
                let text = String::from_utf8_lossy(&self.raw);
                Some(text.trim_end_matches(['\n', '\r']).to_string())
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

/// Call `visit` with every line of the log at `path`.
pub fn for_each_line(path: &Path, mut visit: impl FnMut(&str)) -> ToolResult<()> {
    let mut lines = LogLines::open(path)?;
    for line in lines.by_ref() {
        visit(&line);
    }
    lines.finish()
}

/// Call `visit` with the raw bytes of every line of the log at `path`,
/// without its line ending. Nothing is decoded or allocated per line, which
/// keeps repeated scans of large node logs cheap.
pub fn for_each_line_bytes(path: &Path, mut visit: impl FnMut(&[u8])) -> ToolResult<()> {
    let file = File::open(path).map_err(|error| ToolError::io("open", path, error))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut line = Vec::with_capacity(512);
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| ToolError::io("read", path, error))?;
        if read == 0 {
            return Ok(());
        }
        let mut end = line.len();
        while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        visit(&line[..end]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn log_lines_strip_endings_and_replace_bad_utf8() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(b"first\r\nsec\xffond\nlast")
            .expect("write temp file");
        let lines: Vec<String> = LogLines::open(file.path()).expect("open").collect();
        assert_eq!(lines, vec!["first", "sec\u{fffd}ond", "last"]);
    }
}
