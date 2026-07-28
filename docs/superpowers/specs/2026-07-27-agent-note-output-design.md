# Agent Note Output Design

## Goal

Let people resolve or read an existing Carnet note non-interactively so they can reference it from an AI agent, shell pipeline, or other tool without entering the terminal UI.

## Command contract

Carnet keeps its flag-based, no-subcommand CLI and adds two mutually exclusive modes:

```sh
carnet --path onboarding.md
carnet --print onboarding.md
carnet --repo work --path onboarding.md
carnet --repo work --print onboarding.md
```

`--path` writes the note's absolute path followed by one newline. `--print` writes the note's logical UTF-8 contents and does not add a newline. As in the editor, printed content excludes a UTF-8 BOM and normalizes CRLF line endings to LF while preserving whether the note ends with a newline.

Both flags require `NOTE_PATH`. They may be combined with `--repo NAME` and use the default repository when `--repo` is omitted. Using both flags together is a command-line error. Invocations without either flag keep all existing interactive behavior.

## Routing and components

The CLI parser represents the two flags and selects either the existing interactive launch route or a non-interactive note-output request. Interactive routing remains available to the runtime tests and other library consumers.

A focused note-output component receives the resolved repository, repository-relative note path, output mode, and writer. It opens the existing `Workspace`, resolves the note through the confined path API, and loads it through the same text decoder used by the editor. This keeps repository-boundary, `.git`, directory, and symlink protections in one place and avoids direct arbitrary filesystem reads.

The binary loads the catalog and resolves the request before choosing its execution path. Interactive requests initialize the terminal as they do today. Non-interactive requests call the note-output component and exit without terminal initialization or Git operations.

## Validation and failures

Non-interactive output only succeeds for an existing readable text note. A missing note fails even though interactive Carnet would prepare an unsaved buffer for the same path. Directories, symlinks, binary files, invalid UTF-8, repository-boundary violations, and changed or unavailable repository roots also fail through the existing workspace validation.

Argument parsing, catalog loading, missing repository selection, conflicting flags, and omitted `NOTE_PATH` use exit code `2`. Failures while opening, resolving, loading, or writing a selected note use exit code `1`. Errors are written to stderr with the existing `carnet:` prefix; successful payloads use stdout only.

## Output details

`--path` joins the canonical registered repository root with the workspace-validated relative note path. Its stdout value is intended to be directly readable by people and path-aware agents.

`--print` writes exactly the logical text returned by `LoadedNote::text()`. It uses a writer rather than constructing terminal state, which makes exact output and write failures testable. Neither output mode saves, stages, commits, pushes, or changes catalog state.

## Testing and documentation

Parser and routing tests cover each flag, `--repo` selection, the required note argument, and mutual exclusion. Note-output tests use real temporary repositories and files to cover absolute-path output, exact logical text output, missing notes, and unsafe or non-text targets. Process-level coverage verifies stdout, stderr, and exit status without entering the TUI.

The Clap-generated help block in `docs/cli.md` remains guarded by the existing exact drift test. The surrounding CLI documentation and README include examples aimed at AI-agent and shell use.
