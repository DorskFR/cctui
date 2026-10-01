//! Files to stage into the session once it is up.
//!
//! The spawn request carries no bytes: the files are uploaded to the
//! registered session, which is the same path a mid-conversation attachment
//! takes, so one upload route serves both.

use std::path::{Path, PathBuf};

use cctui_clientcore::spawn::SpawnFields;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};

use super::SpawnSection;
use crate::app::action::Effect;
use crate::app::attach::Attachment;
use crate::theme;

#[derive(Debug, Default)]
pub struct FilesSection {
    pub staged: Vec<Attachment>,
    /// Where the files came from, for the request's draft-only name list and
    /// for re-reading at launch.
    pub paths: Vec<PathBuf>,
    pub cursor: usize,
    /// The path being typed, when the input is open.
    pub input: Option<String>,
    pub error: Option<String>,
}

impl FilesSection {
    pub fn open_input(&mut self) {
        self.input = Some(String::new());
        self.error = None;
    }

    /// Stage bytes an effect already read and gated against the upload caps.
    pub fn staged_read(&mut self, path: &Path, bytes: Vec<u8>, content_type: String) {
        let name = path
            .file_name()
            .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        self.staged.push(Attachment { name, bytes, content_type, dimensions: None });
        self.paths.push(path.to_path_buf());
        self.error = None;
    }

    pub fn read_failed(&mut self, message: String) {
        self.error = Some(message);
    }

    pub fn remove_at_cursor(&mut self) {
        if self.cursor < self.staged.len() {
            self.staged.remove(self.cursor);
            self.paths.remove(self.cursor);
            self.cursor = self.cursor.min(self.staged.len().saturating_sub(1));
        }
    }

    /// Names only — all a draft may remember of a file.
    fn fill_names(&self, req: &mut cctui_proto::api::SpawnRequest) {
        req.attachment_names = self.staged.iter().map(|a| a.name.clone()).collect();
    }

    /// The multipart parts the spawn carries, in the order they were added.
    #[must_use]
    pub fn staged_parts(&self) -> Vec<(String, Vec<u8>)> {
        self.staged.iter().map(|a| (a.name.clone(), a.bytes.clone())).collect()
    }

    /// The upload the dialog runs against the session it just spawned.
    #[allow(dead_code, reason = "the dialog's submit path calls this; it lands with the skeleton")]
    #[must_use]
    pub fn upload_effect(&self, session_id: &str) -> Option<Effect> {
        if self.staged.is_empty() {
            return None;
        }
        Some(Effect::UploadAttachments {
            session_id: session_id.to_owned(),
            files: self.staged.iter().map(|a| (a.name.clone(), a.bytes.clone())).collect(),
            content: String::new(),
        })
    }

    fn own_lines(&self, focused: Option<usize>) -> Vec<Line<'static>> {
        let marker = if focused.is_some() { "›" } else { " " };
        let mut out = vec![Line::from(vec![
            Span::styled(format!(" {marker} "), theme::section_title()),
            Span::styled(
                if self.staged.is_empty() {
                    "none  (o add path)".to_owned()
                } else {
                    format!("{} staged  (o add · d remove)", self.staged.len())
                },
                theme::dim(),
            ),
        ])];
        for (i, file) in self.staged.iter().enumerate() {
            let style = if focused.is_some() && i == self.cursor {
                theme::selected()
            } else {
                theme::dim()
            };
            out.push(Line::from(Span::styled(format!("      {}", file.chip_label()), style)));
        }
        if let Some(input) = &self.input {
            out.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(format!("path: {input}_"), theme::bold()),
            ]));
        }
        if let Some(error) = &self.error {
            out.push(Line::from(Span::styled(format!("      ! {error}"), theme::error())));
        }
        out
    }

    fn own_handle(&mut self, key: KeyEvent) -> Vec<Effect> {
        if let Some(input) = self.input.as_mut() {
            match key.code {
                KeyCode::Char(c) => input.push(c),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => {
                    let typed = std::mem::take(input);
                    self.input = None;
                    let trimmed = typed.trim();
                    if trimmed.is_empty() {
                        return Vec::new();
                    }
                    // The bytes are read by an effect: a path can be a multi-GB
                    // file or a FIFO, and this runs on the input/render thread.
                    self.error = None;
                    return vec![Effect::ReadSpawnFile {
                        path: expand_tilde(trimmed).display().to_string(),
                    }];
                }
                KeyCode::Esc => self.input = None,
                _ => {}
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Char('o') => self.open_input(),
            KeyCode::Char('d') => self.remove_at_cursor(),
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(self.staged.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            _ => {}
        }
        Vec::new()
    }
}

impl SpawnSection for FilesSection {
    fn as_files_mut(&mut self) -> Option<&mut Self> {
        Some(self)
    }

    fn title(&self) -> &'static str {
        "Files"
    }

    fn rows(&self, _fields: &SpawnFields) -> usize {
        1
    }

    fn lines(
        &self,
        focused: Option<usize>,
        width: u16,
        _fields: &SpawnFields,
    ) -> Vec<Line<'static>> {
        super::clamp_rows(self.own_lines(focused), width)
    }

    fn handle(&mut self, _row: usize, key: KeyEvent, _fields: &mut SpawnFields) -> Vec<Effect> {
        self.own_handle(key)
    }

    fn apply(&self, request: &mut cctui_proto::api::SpawnRequest) {
        self.fill_names(request);
    }

    fn problems(&self) -> Vec<String> {
        self.error.iter().cloned().collect()
    }

    fn parts(&self) -> Vec<(String, Vec<u8>)> {
        self.staged_parts()
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    let Some(rest) = path.strip_prefix("~/") else { return PathBuf::from(path) };
    dirs::home_dir().map_or_else(|| PathBuf::from(path), |home| home.join(rest))
}

#[cfg(test)]
mod tests {
    use cctui_clientcore::spawn::SpawnFields;

    use super::{FilesSection, SpawnSection, expand_tilde};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn request() -> cctui_proto::api::SpawnRequest {
        serde_json::from_value(serde_json::json!({"machine_id": "m", "working_dir": "/w"}))
            .expect("a request")
    }

    /// Stage a path the way the effect's reply does, so the tests exercise the
    /// same seam the dialog uses.
    fn staged(section: &mut FilesSection, path: &std::path::Path) {
        let bytes = std::fs::read(path).expect("read");
        section.staged_read(path, bytes, "text/plain".to_owned());
    }

    fn typed(section: &mut FilesSection, text: &str) {
        let mut fields = SpawnFields::default();
        for c in text.chars() {
            section.handle(0, key(KeyCode::Char(c)), &mut fields);
        }
    }

    #[test]
    fn a_typed_path_is_read_and_chipped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("trace.log");
        std::fs::write(&path, b"0123456789").expect("write");

        let mut s = FilesSection::default();
        s.handle(0, key(KeyCode::Char('o')), &mut SpawnFields::default());
        typed(&mut s, path.to_str().expect("utf8"));
        // Enter asks for the bytes rather than reading them on this thread.
        match s.handle(0, key(KeyCode::Enter), &mut SpawnFields::default()).as_slice() {
            [crate::app::action::Effect::ReadSpawnFile { path: asked }] => {
                assert_eq!(asked, path.to_str().expect("utf8"));
            }
            other => panic!("expected one read effect, got {}", other.len()),
        }
        assert!(s.staged.is_empty(), "nothing is staged until the bytes land");
        staged(&mut s, &path);

        assert_eq!(s.staged.len(), 1);
        assert_eq!(s.staged[0].name, "trace.log");
        assert_eq!(s.staged[0].bytes.len(), 10);
        assert!(s.error.is_none());

        let rendered: String = s
            .own_lines(Some(0))
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.to_string()))
            .collect();
        assert!(rendered.contains("trace.log"), "{rendered}");
    }

    #[test]
    fn the_request_carries_names_not_bytes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("notes.md");
        std::fs::write(&path, b"hi").expect("write");
        let mut s = FilesSection::default();
        staged(&mut s, &path);

        let mut req = request();
        s.apply(&mut req);
        assert_eq!(req.attachment_names, ["notes.md"]);
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(!json.contains("hi\""), "no bytes ride along: {json}");
    }

    #[test]
    fn the_upload_goes_to_the_session_that_was_spawned() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.txt");
        std::fs::write(&path, b"x").expect("write");
        let mut s = FilesSection::default();
        assert!(s.upload_effect("s-1").is_none(), "nothing staged, nothing to upload");

        staged(&mut s, &path);
        match s.upload_effect("s-1") {
            Some(crate::app::action::Effect::UploadAttachments { session_id, files, .. }) => {
                assert_eq!(session_id, "s-1");
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].0, "a.txt");
            }
            _ => panic!("expected an upload effect"),
        }
    }

    #[test]
    fn a_path_that_is_not_there_says_so_and_stages_nothing() {
        let mut s = FilesSection::default();
        s.read_failed("cannot read /nope/missing.txt: No such file".to_owned());
        assert!(s.staged.is_empty());
        assert!(s.error.as_deref().expect("an error").contains("missing.txt"));
    }

    #[test]
    fn a_file_can_be_taken_back_off() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for name in ["one.txt", "two.txt"] {
            std::fs::write(tmp.path().join(name), b"x").expect("write");
        }
        let mut s = FilesSection::default();
        staged(&mut s, &tmp.path().join("one.txt"));
        staged(&mut s, &tmp.path().join("two.txt"));
        s.cursor = 0;
        s.handle(0, key(KeyCode::Char('d')), &mut SpawnFields::default());
        assert_eq!(s.staged.len(), 1);
        assert_eq!(s.staged[0].name, "two.txt");
        assert_eq!(s.paths.len(), 1, "the path list stays in step");
    }

    #[test]
    fn escape_abandons_the_path_input() {
        let mut s = FilesSection::default();
        s.handle(0, key(KeyCode::Char('o')), &mut SpawnFields::default());
        typed(&mut s, "/tmp/x");
        s.handle(0, key(KeyCode::Esc), &mut SpawnFields::default());
        assert!(s.input.is_none());
        assert!(s.staged.is_empty());
    }

    #[test]
    fn a_leading_tilde_resolves_against_home() {
        let expanded = expand_tilde("~/x.txt");
        assert!(expanded.is_absolute(), "{expanded:?}");
        assert!(expanded.ends_with("x.txt"));
        assert_eq!(expand_tilde("/abs/x"), std::path::Path::new("/abs/x"));
    }
}
