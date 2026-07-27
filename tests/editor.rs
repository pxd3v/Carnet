use std::{fs, path::Path};

use carnet::{
    catalog::RepoEntry,
    editor::{
        Clipboard, ClipboardError, EDITOR_HISTORY_ENTRY_LIMIT, Editor, EditorCommand,
        EditorOutcome, HighlightLanguage, Motion,
    },
    workspace::{FileError, FileOperation, FileOutcome, NewlineStyle, Workspace},
};
use tempfile::tempdir;
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

use proptest::prelude::*;

#[test]
fn complete_undo_saves_with_original_metadata_and_retains_conflict_detection() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    let note_path = root.join("note.md");
    let original_bytes = b"\xef\xbb\xbffirst\r\nsecond\r\n";
    fs::write(&note_path, original_bytes).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&note_path, fs::Permissions::from_mode(0o640)).unwrap();
    let workspace = open_workspace(root);
    let note = workspace
        .load_note(&workspace.resolve_note(Path::new("note.md")).unwrap())
        .unwrap();
    let expected_hash = note.content_hash();

    let mut editor = Editor::from_loaded(note);

    assert_eq!(editor.text(), "first\nsecond\n");
    assert!(!editor.is_dirty());
    editor.apply(EditorCommand::Insert("temporary".into()));
    editor.apply(EditorCommand::Undo);
    assert_eq!(editor.text(), "first\nsecond\n");
    assert!(!editor.is_dirty());
    let operation = editor.save_operation(false);
    match &operation {
        FileOperation::Save {
            note,
            content,
            overwrite,
        } => {
            assert_eq!(note.path().relative(), Path::new("note.md"));
            assert_eq!(note.content_hash(), expected_hash);
            assert!(note.has_bom());
            assert_eq!(note.newline_style(), NewlineStyle::CrLf);
            assert!(note.had_final_newline());
            assert_eq!(content, "first\nsecond\n");
            assert!(!overwrite);
        }
        operation => panic!("unexpected operation: {operation:?}"),
    }

    let FileOutcome::Saved(saved) = Workspace::apply(operation).unwrap() else {
        panic!("expected saved note");
    };
    assert_eq!(fs::read(&note_path).unwrap(), original_bytes);
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&note_path).unwrap().permissions().mode() & 0o777,
        0o640
    );

    fs::write(&note_path, b"\xef\xbb\xbfexternal\r\n").unwrap();
    let error = Workspace::apply(Editor::from_loaded(saved).save_operation(false)).unwrap_err();
    assert!(matches!(error, FileError::ExternalModification { .. }));
    assert_eq!(fs::read(&note_path).unwrap(), b"\xef\xbb\xbfexternal\r\n");
}

#[test]
fn horizontal_motion_and_shift_selection_never_split_graphemes() {
    let mut editor = editor_from("unicode.md", "a e\u{301}好👩‍👩‍👧‍👦🇺🇳");

    assert_eq!(
        editor.apply(move_command(Motion::Right, false)),
        EditorOutcome::Moved
    );
    assert_eq!(editor.cursor(), 1);
    editor.apply(move_command(Motion::Right, false));
    assert_eq!(editor.cursor(), 2);
    editor.apply(move_command(Motion::Right, false));
    assert_eq!(editor.cursor(), 4);

    editor.apply(move_command(Motion::Right, true));
    assert_eq!(editor.selected_text().as_deref(), Some("好"));
    editor.apply(move_command(Motion::Right, true));
    assert_eq!(editor.selected_text().as_deref(), Some("好👩‍👩‍👧‍👦"));
    editor.apply(move_command(Motion::Left, false));
    assert_eq!(editor.selected_text(), None);
    assert_eq!(editor.cursor(), 4);
}

#[test]
fn vertical_motion_preserves_a_visual_column_across_short_lines() {
    let mut editor = editor_from("columns.md", "a好z\n12\nabcdef");
    editor.apply(move_command(Motion::LineEnd, false));
    assert_eq!(editor.cursor(), 3);

    editor.apply(move_command(Motion::Down, false));
    assert_eq!(editor.cursor(), 6);
    editor.apply(move_command(Motion::Down, false));
    assert_eq!(editor.cursor(), 11);
    editor.apply(move_command(Motion::DocumentStart, false));
    assert_eq!(editor.cursor(), 0);
    editor.apply(move_command(Motion::DocumentEnd, true));
    assert_eq!(editor.selected_text().as_deref(), Some("a好z\n12\nabcdef"));
}

#[test]
fn word_motion_skips_spacing_and_punctuation_on_unicode_boundaries() {
    let mut editor = editor_from("words.md", "one  café, 世界");

    editor.apply(move_command(Motion::WordRight, false));
    assert_eq!(editor.cursor(), 3);
    editor.apply(move_command(Motion::WordRight, false));
    assert_eq!(editor.cursor(), 9);
    editor.apply(move_command(Motion::WordRight, true));
    assert_eq!(editor.selected_text().as_deref(), Some(", 世"));
    editor.apply(move_command(Motion::WordLeft, false));
    assert_eq!(editor.cursor(), 9);
    editor.apply(move_command(Motion::WordLeft, false));
    assert_eq!(editor.cursor(), 5);
}

#[test]
fn insertion_deletion_and_newline_replace_whole_selections() {
    let mut editor = editor_from("edit.md", "one👩‍🚀two");
    editor.apply(move_command(Motion::Right, true));
    editor.apply(move_command(Motion::Right, true));
    editor.apply(EditorCommand::Insert("X".into()));
    assert_eq!(editor.text(), "Xe👩‍🚀two");
    assert_eq!(editor.selected_text(), None);

    editor.apply(EditorCommand::Delete);
    assert_eq!(editor.text(), "X👩‍🚀two");
    editor.apply(EditorCommand::Backspace);
    assert_eq!(editor.text(), "👩‍🚀two");
    editor.apply(EditorCommand::Newline);
    assert_eq!(editor.text(), "\n👩‍🚀two");
    assert!(editor.is_dirty());
}

#[test]
fn identical_replacement_collapses_the_selection_before_the_next_insert() {
    let mut editor = editor_from("identical.md", "x");
    editor.apply(move_command(Motion::Right, true));

    assert_eq!(editor.selected_text().as_deref(), Some("x"));
    assert_eq!(
        editor.apply(EditorCommand::Insert("x".into())),
        EditorOutcome::Moved
    );
    assert_eq!(editor.text(), "x");
    assert_eq!(editor.selection(), None);
    assert_eq!(editor.cursor(), 1);
    assert!(!editor.is_dirty());

    editor.apply(EditorCommand::Insert("y".into()));
    assert_eq!(editor.text(), "xy");
}

#[test]
fn edits_are_single_transactions_and_new_edits_clear_redo() {
    let mut editor = editor_from("history.md", "base");
    editor.apply(EditorCommand::Insert("a".into()));
    editor.apply(EditorCommand::Insert("b".into()));
    assert_eq!(editor.text(), "abbase");

    editor.apply(EditorCommand::Undo);
    assert_eq!(editor.text(), "abase");
    editor.apply(EditorCommand::Undo);
    assert_eq!(editor.text(), "base");
    assert!(!editor.is_dirty());
    editor.apply(EditorCommand::Redo);
    assert_eq!(editor.text(), "abase");
    editor.apply(EditorCommand::Insert("x".into()));
    assert_eq!(editor.apply(EditorCommand::Redo), EditorOutcome::NoChange);
    assert_eq!(editor.text(), "axbase");
}

#[test]
fn large_note_history_stays_bounded_and_full_undo_reaches_the_loaded_baseline() {
    let baseline = "x".repeat(32 * 1024);
    let mut editor = editor_from("large-history.md", &baseline);
    let edit_count = EDITOR_HISTORY_ENTRY_LIMIT * 2;

    for _ in 0..edit_count {
        assert_eq!(
            editor.apply(EditorCommand::Insert("y".into())),
            EditorOutcome::Changed
        );
        assert!(editor.history_entry_count() <= EDITOR_HISTORY_ENTRY_LIMIT);
    }

    let mut undo_count = 0;
    while editor.apply(EditorCommand::Undo) == EditorOutcome::Changed {
        undo_count += 1;
        assert!(editor.history_entry_count() <= EDITOR_HISTORY_ENTRY_LIMIT);
    }
    assert!(undo_count <= EDITOR_HISTORY_ENTRY_LIMIT);
    assert_eq!(editor.text(), baseline);
    assert!(!editor.is_dirty());
}

#[test]
fn bracketed_multiline_paste_is_one_undo_transaction() {
    let mut editor = editor_from("paste.md", "tail");

    editor.apply(EditorCommand::BracketedPaste("one\r\ntwo\n".into()));
    assert_eq!(editor.text(), "one\ntwo\ntail");
    editor.apply(EditorCommand::Undo);
    assert_eq!(editor.text(), "tail");
    assert!(!editor.is_dirty());
    editor.apply(EditorCommand::Redo);
    assert_eq!(editor.text(), "one\ntwo\ntail");
}

#[test]
fn clipboard_falls_back_locally_when_the_injected_boundary_fails() {
    let mut editor =
        editor_from_with_clipboard("clipboard.md", "copy me", Box::new(FailingClipboard));
    editor.apply(EditorCommand::SelectAll);
    assert_eq!(editor.apply(EditorCommand::Copy), EditorOutcome::Copied);
    editor.apply(move_command(Motion::DocumentEnd, false));
    editor.apply(EditorCommand::Paste);
    assert_eq!(editor.text(), "copy mecopy me");

    editor.apply(EditorCommand::SelectAll);
    assert_eq!(editor.apply(EditorCommand::Cut), EditorOutcome::Changed);
    assert_eq!(editor.text(), "");
    editor.apply(EditorCommand::Paste);
    assert_eq!(editor.text(), "copy mecopy me");
}

#[test]
fn multiline_indent_and_outdent_are_atomic() {
    let mut editor = editor_from("indent.md", "one\n  two\n\tthree\nfour");
    editor.apply(EditorCommand::SelectAll);
    editor.apply(EditorCommand::Indent);
    assert_eq!(editor.text(), "    one\n      two\n    \tthree\n    four");
    editor.apply(EditorCommand::Undo);
    assert_eq!(editor.text(), "one\n  two\n\tthree\nfour");

    editor.apply(EditorCommand::Outdent);
    assert_eq!(editor.text(), "one\ntwo\nthree\nfour");
    editor.apply(EditorCommand::Undo);
    assert_eq!(editor.text(), "one\n  two\n\tthree\nfour");
}

#[test]
fn literal_find_navigates_forward_backward_and_wraps() {
    let mut editor = editor_from("find.md", "α cat\ncathedral\ncat");
    editor.apply(EditorCommand::SetFindQuery("cat".into()));

    assert_eq!(
        editor.apply(EditorCommand::FindNext),
        EditorOutcome::SearchMatch {
            current: 1,
            total: 3,
        }
    );
    assert_eq!(editor.selection(), Some(2..5));
    assert_eq!(editor.selected_text().as_deref(), Some("cat"));
    assert_eq!(
        editor.apply(EditorCommand::FindNext),
        EditorOutcome::SearchMatch {
            current: 2,
            total: 3,
        }
    );
    assert_eq!(editor.selection(), Some(6..9));
    editor.apply(EditorCommand::FindPrevious);
    assert_eq!(editor.selection(), Some(2..5));
    assert_eq!(
        editor.apply(EditorCommand::FindPrevious),
        EditorOutcome::SearchMatch {
            current: 3,
            total: 3,
        }
    );
    assert_eq!(editor.selection(), Some(16..19));
    assert!(!editor.is_dirty());
}

#[test]
fn select_all_resets_find_navigation_to_the_new_cursor() {
    let mut editor = editor_from("find.md", "cat one cat two");
    editor.apply(EditorCommand::SetFindQuery("cat".into()));
    editor.apply(EditorCommand::FindNext);
    assert_eq!(editor.selection(), Some(0..3));

    editor.apply(EditorCommand::SelectAll);
    assert_eq!(editor.selection(), Some(0..15));

    assert_eq!(
        editor.apply(EditorCommand::FindNext),
        EditorOutcome::SearchMatch {
            current: 1,
            total: 2,
        }
    );
    assert_eq!(editor.selection(), Some(0..3));
}

#[test]
fn find_next_starts_at_or_after_the_current_cursor() {
    let mut editor = editor_from("find.md", "cat cat");
    for _ in 0..4 {
        editor.apply(move_command(Motion::Right, false));
    }
    editor.apply(EditorCommand::SetFindQuery("cat".into()));

    assert_eq!(
        editor.apply(EditorCommand::FindNext),
        EditorOutcome::SearchMatch {
            current: 2,
            total: 2,
        }
    );
    assert_eq!(editor.selection(), Some(4..7));
}

#[test]
fn find_does_not_create_an_invalid_selection_inside_a_grapheme() {
    let mut editor = editor_from("find.md", "e\u{301}");
    editor.apply(EditorCommand::SetFindQuery("\u{301}".into()));

    assert_eq!(
        editor.apply(EditorCommand::FindNext),
        EditorOutcome::NoChange
    );
    assert_eq!(editor.selection(), None);
}

#[test]
fn highlighting_selects_markdown_and_html_by_extension_and_keeps_other_files_plain() {
    let mut markdown = editor_from("note.MD", "# Heading\n\nText with *emphasis*.\n");
    let mut html = editor_from("page.html", "<h1>Heading</h1>\n<p>Text</p>\n");
    let mut text = editor_from("note.txt", "# not highlighted\n");
    let mut other = editor_from("note.rst", "Heading\n=======\n");

    assert_eq!(markdown.highlight_language(), HighlightLanguage::Markdown);
    assert_eq!(html.highlight_language(), HighlightLanguage::Html);
    assert_eq!(text.highlight_language(), HighlightLanguage::PlainText);
    assert_eq!(other.highlight_language(), HighlightLanguage::PlainText);
    assert!(!markdown.highlighted_spans().is_empty());
    assert!(!html.highlighted_spans().is_empty());
    assert!(text.highlighted_spans().is_empty());
    assert!(other.highlighted_spans().is_empty());
}

#[test]
fn highlight_cache_recomputes_styled_ranges_after_an_edit() {
    let mut editor = editor_from("cache.md", "# One\n");
    let original_end = editor
        .highlighted_spans()
        .last()
        .map(|span| span.range.end)
        .unwrap();
    assert_eq!(original_end, 6);

    editor.apply(move_command(Motion::DocumentEnd, false));
    editor.apply(EditorCommand::Insert("\n**Two**".into()));
    let expected_end = editor.text().chars().count();
    let spans = editor.highlighted_spans();

    assert_eq!(spans.last().map(|span| span.range.end), Some(expected_end));
    assert!(spans.iter().all(|span| !span.range.is_empty()));
    assert!(spans.iter().any(|span| span.style.foreground[3] > 0));
}

#[test]
fn markdown_highlights_heading_and_emphasis_differently_from_plain_text() {
    let mut editor = editor_from("tokens.md", "# Heading\nplain *emphasis* tail\n");
    let spans = editor.highlighted_spans().to_vec();
    let plain = style_at(&spans, 10);

    assert_ne!(style_at(&spans, 2), plain);
    assert_ne!(style_at(&spans, 17), plain);
}

#[test]
fn html_highlights_tag_names_differently_from_adjacent_text() {
    let mut editor = editor_from("tokens.html", "<p>plain</p>");
    let spans = editor.highlighted_spans().to_vec();

    assert_ne!(style_at(&spans, 1), style_at(&spans, 3));
    assert_ne!(style_at(&spans, 10), style_at(&spans, 3));
}

#[test]
fn outdent_keeps_selection_endpoints_valid_when_indented_lines_are_adjacent() {
    let mut editor = editor_from("outdent.md", "    \n x");
    editor.apply(EditorCommand::SelectAll);

    editor.apply(EditorCommand::Outdent);

    assert_eq!(editor.text(), "\nx");
    assert_eq!(editor.cursor(), 2);
    assert_eq!(editor.selected_text().as_deref(), Some("\nx"));
}

#[test]
fn deletion_snaps_the_cursor_when_surrounding_text_forms_a_new_grapheme() {
    let mut backspace = editor_from("backspace.md", "🇺 🇳");
    backspace.apply(move_command(Motion::Right, false));
    backspace.apply(move_command(Motion::Right, false));
    backspace.apply(EditorCommand::Backspace);
    assert_eq!(backspace.text(), "🇺🇳");
    assert_eq!(backspace.cursor(), 2);

    let mut delete = editor_from("delete.md", "🇺 🇳");
    delete.apply(move_command(Motion::Right, false));
    delete.apply(EditorCommand::Delete);
    assert_eq!(delete.text(), "🇺🇳");
    assert_eq!(delete.cursor(), 2);
}

#[test]
fn indent_snaps_endpoints_created_inside_a_spacing_mark_grapheme() {
    let mut editor = editor_from("indent-boundary.md", "");
    editor.apply(EditorCommand::Insert("ၖ".into()));
    editor.apply(move_command(Motion::DocumentStart, false));

    editor.apply(EditorCommand::Indent);

    assert_eq!(editor.text(), "    ၖ");
    assert_valid_editor_endpoints(&editor);
    editor.apply(EditorCommand::Undo);
    assert_valid_editor_endpoints(&editor);
    editor.apply(EditorCommand::Redo);
    assert_eq!(editor.text(), "    ၖ");
    assert_valid_editor_endpoints(&editor);
}

#[test]
fn indent_snaps_both_ends_of_a_selection_around_a_spacing_mark_grapheme() {
    let mut editor = editor_from("indent-selection.md", "ၖ");
    editor.apply(EditorCommand::SelectAll);

    editor.apply(EditorCommand::Indent);

    assert_eq!(editor.text(), "    ၖ");
    assert_valid_editor_endpoints(&editor);
    editor.apply(EditorCommand::Undo);
    assert_valid_editor_endpoints(&editor);
    editor.apply(EditorCommand::Redo);
    assert_valid_editor_endpoints(&editor);
}

proptest! {
    #[test]
    fn generated_unicode_actions_keep_endpoints_on_graphemes_and_fully_undo(
        original in unicode_text(),
        actions in prop::collection::vec((0_u8..16, any::<bool>(), unicode_text()), 0..48),
    ) {
        let mut editor = editor_from("generated.md", &original);
        let baseline = editor.text();
        let mut changed_transactions = 0;

        assert_valid_editor_endpoints(&editor);
        for (kind, extend, inserted) in actions {
            let command = match kind {
                0 => EditorCommand::Insert(inserted),
                1 => EditorCommand::BracketedPaste(inserted),
                2 => EditorCommand::Newline,
                3 => EditorCommand::Backspace,
                4 => EditorCommand::Delete,
                5 => EditorCommand::Indent,
                6 => EditorCommand::Outdent,
                7 => move_command(Motion::Left, extend),
                8 => move_command(Motion::Right, extend),
                9 => move_command(Motion::Up, extend),
                10 => move_command(Motion::Down, extend),
                11 => move_command(Motion::LineStart, extend),
                12 => move_command(Motion::LineEnd, extend),
                13 => move_command(Motion::DocumentStart, extend),
                14 => move_command(Motion::DocumentEnd, extend),
                _ => EditorCommand::SelectAll,
            };
            let before = editor.text();
            editor.apply(command);
            if editor.text() != before {
                changed_transactions += 1;
            }
            assert_valid_editor_endpoints(&editor);
        }

        for _ in 0..changed_transactions {
            prop_assert_eq!(editor.apply(EditorCommand::Undo), EditorOutcome::Changed);
            assert_valid_editor_endpoints(&editor);
        }
        prop_assert_eq!(editor.text(), baseline);
        prop_assert!(!editor.is_dirty());
        prop_assert_eq!(editor.apply(EditorCommand::Undo), EditorOutcome::NoChange);
    }
}

fn open_workspace(root: std::path::PathBuf) -> Workspace {
    Workspace::open(RepoEntry {
        id: Uuid::new_v4(),
        name: "notes".into(),
        path: root,
    })
    .unwrap()
}

fn editor_from(name: &str, text: &str) -> Editor {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::write(root.join(name), text).unwrap();
    let workspace = open_workspace(root);
    let note = workspace
        .load_note(&workspace.resolve_note(Path::new(name)).unwrap())
        .unwrap();
    Editor::from_loaded(note)
}

fn editor_from_with_clipboard(name: &str, text: &str, clipboard: Box<dyn Clipboard>) -> Editor {
    let sandbox = tempdir().unwrap();
    let root = fs::canonicalize(sandbox.path()).unwrap();
    fs::write(root.join(name), text).unwrap();
    let workspace = open_workspace(root);
    let note = workspace
        .load_note(&workspace.resolve_note(Path::new(name)).unwrap())
        .unwrap();
    Editor::from_loaded_with_clipboard(note, clipboard)
}

struct FailingClipboard;

impl Clipboard for FailingClipboard {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        Err(ClipboardError::Unavailable)
    }

    fn write_text(&mut self, _text: &str) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unavailable)
    }
}

fn move_command(motion: Motion, extend_selection: bool) -> EditorCommand {
    EditorCommand::Move {
        motion,
        extend_selection,
    }
}

fn unicode_text() -> BoxedStrategy<String> {
    prop::collection::vec(
        prop_oneof![
            5 => any::<char>().prop_filter("editor text excludes NUL and CR", |character| {
                *character != '\0' && *character != '\r'
            }).prop_map(|character| character.to_string()),
            1 => Just("e\u{301}".to_owned()),
            1 => Just("好".to_owned()),
            1 => Just("👩‍👩‍👧‍👦".to_owned()),
            1 => Just("🇺🇳".to_owned()),
            1 => Just("ၖ".to_owned()),
            1 => Just("\n".to_owned()),
        ],
        0..32,
    )
    .prop_map(|pieces| pieces.concat())
    .boxed()
}

fn assert_valid_editor_endpoints(editor: &Editor) {
    let text = editor.text();
    assert!(is_grapheme_boundary(&text, editor.cursor()));
    if let Some(selection) = editor.selection() {
        assert!(is_grapheme_boundary(&text, selection.start));
        assert!(is_grapheme_boundary(&text, selection.end));
    }
}

fn is_grapheme_boundary(text: &str, char_index: usize) -> bool {
    let byte_index = text
        .char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte);
    char_index <= text.chars().count()
        && (byte_index == text.len()
            || text
                .grapheme_indices(true)
                .any(|(boundary, _)| boundary == byte_index))
}

fn style_at(
    spans: &[carnet::editor::HighlightSpan],
    char_index: usize,
) -> carnet::editor::HighlightStyle {
    spans
        .iter()
        .find(|span| span.range.contains(&char_index))
        .unwrap_or_else(|| panic!("no highlighted span at scalar index {char_index}"))
        .style
}
