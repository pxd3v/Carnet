use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use carnet::{
    catalog::RepoEntry,
    cli::{NoteOutputRequest, OutputMode},
    note_output::{NoteOutputError, write_note_output},
    workspace::{FileError, PathError},
};
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

struct Fixture {
    _sandbox: TempDir,
    repository: RepoEntry,
    root: PathBuf,
}

impl Fixture {
    fn empty() -> Self {
        let sandbox = tempdir().unwrap();
        let root = fs::canonicalize(sandbox.path()).unwrap();
        let repository = RepoEntry {
            id: Uuid::new_v4(),
            name: "notes".to_owned(),
            path: root.clone(),
        };
        Self {
            _sandbox: sandbox,
            repository,
            root,
        }
    }

    fn with_note(path: &str, contents: &[u8]) -> Self {
        let fixture = Self::empty();
        let absolute = fixture.root.join(path);
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(absolute, contents).unwrap();
        fixture
    }

    fn request(&self, note: &str, mode: OutputMode) -> NoteOutputRequest {
        NoteOutputRequest {
            repository: self.repository.clone(),
            note: PathBuf::from(note),
            mode,
        }
    }
}

#[test]
fn path_mode_writes_the_absolute_existing_note_path_with_one_newline() {
    let fixture = Fixture::with_note("onboarding.md", b"hello");
    let mut output = Vec::new();

    write_note_output(
        fixture.request("onboarding.md", OutputMode::Path),
        &mut output,
    )
    .unwrap();

    assert_eq!(
        String::from_utf8(output).unwrap(),
        format!("{}\n", fixture.root.join("onboarding.md").display())
    );
}

#[test]
fn print_mode_writes_logical_text_without_adding_a_newline() {
    let fixture = Fixture::with_note("onboarding.md", b"\xef\xbb\xbffirst\r\nsecond");
    let mut output = Vec::new();

    write_note_output(
        fixture.request("onboarding.md", OutputMode::Print),
        &mut output,
    )
    .unwrap();

    assert_eq!(output, b"first\nsecond");
}

#[test]
fn a_missing_note_fails_instead_of_emitting_a_prospective_reference() {
    let fixture = Fixture::empty();

    let error = write_note_output(
        fixture.request("missing.md", OutputMode::Path),
        &mut Vec::new(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        NoteOutputError::Missing { path } if path == Path::new("missing.md")
    ));
}

#[test]
fn binary_and_invalid_utf8_notes_are_rejected() {
    let binary = Fixture::with_note("binary.md", b"text\0data");
    let invalid = Fixture::with_note("invalid.md", &[0xff]);

    let binary_error = write_note_output(
        binary.request("binary.md", OutputMode::Print),
        &mut Vec::new(),
    )
    .unwrap_err();
    let invalid_error = write_note_output(
        invalid.request("invalid.md", OutputMode::Print),
        &mut Vec::new(),
    )
    .unwrap_err();

    assert!(matches!(
        binary_error,
        NoteOutputError::File(FileError::Binary { .. })
    ));
    assert!(matches!(
        invalid_error,
        NoteOutputError::File(FileError::InvalidUtf8 { .. })
    ));
}

#[test]
fn directories_and_symbolic_links_are_rejected() {
    let directory = Fixture::empty();
    fs::create_dir(directory.root.join("folder")).unwrap();
    let symlink = Fixture::with_note("target.md", b"target");
    std::os::unix::fs::symlink("target.md", symlink.root.join("link.md")).unwrap();

    let directory_error = write_note_output(
        directory.request("folder", OutputMode::Path),
        &mut Vec::new(),
    )
    .unwrap_err();
    let symlink_error = write_note_output(
        symlink.request("link.md", OutputMode::Path),
        &mut Vec::new(),
    )
    .unwrap_err();

    assert!(matches!(
        directory_error,
        NoteOutputError::Path(PathError::DirectoryTarget { .. })
    ));
    assert!(matches!(
        symlink_error,
        NoteOutputError::Path(PathError::Symlink { .. })
    ));
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn output_write_failures_identify_the_selected_note() {
    let fixture = Fixture::with_note("onboarding.md", b"hello");

    let error = write_note_output(
        fixture.request("onboarding.md", OutputMode::Print),
        &mut FailingWriter,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        NoteOutputError::Write { path, source }
            if path == Path::new("onboarding.md")
                && source.kind() == io::ErrorKind::BrokenPipe
    ));
}
