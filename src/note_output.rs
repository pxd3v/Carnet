use std::{io::Write, path::PathBuf};

use thiserror::Error;

use crate::{
    cli::{NoteOutputRequest, OutputMode},
    workspace::{FileError, PathError, Workspace, WorkspaceError},
};

#[derive(Debug, Error)]
pub enum NoteOutputError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Path(#[from] PathError),
    #[error(transparent)]
    File(#[from] FileError),
    #[error("note does not exist: {path}")]
    Missing { path: PathBuf },
    #[error("could not write output for {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn write_note_output(
    request: NoteOutputRequest,
    writer: &mut impl Write,
) -> Result<(), NoteOutputError> {
    let workspace = Workspace::open(request.repository)?;
    let path = workspace.resolve_note(&request.note)?;
    let note = workspace.load_note(&path)?;
    if !note.is_saved() {
        return Err(NoteOutputError::Missing { path: request.note });
    }

    let result = match request.mode {
        OutputMode::Path => writeln!(
            writer,
            "{}",
            workspace.root().join(note.path().relative()).display()
        ),
        OutputMode::Print => writer.write_all(note.text().as_bytes()),
    };
    result.map_err(|source| NoteOutputError::Write {
        path: request.note,
        source,
    })
}
