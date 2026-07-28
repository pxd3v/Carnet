# Ghostty setup for native macOS editing

Carnet supports portable `Control` shortcuts without terminal configuration. To use macOS-native `Command`, `Option`, and `Shift` editing shortcuts, Ghostty must release a few keys that it handles before terminal applications can see them.

Add this block to your Ghostty configuration:

```text
# Let terminal editors handle native text-editing shortcuts.
keybind = shift+arrow_left=unbind
keybind = shift+arrow_right=unbind
keybind = shift+arrow_up=unbind
keybind = shift+arrow_down=unbind
keybind = super+c=unbind
keybind = super+a=unbind
keybind = super+z=unbind
keybind = super+shift+z=unbind
keybind = super+f=unbind
keybind = super+arrow_left=unbind
keybind = super+arrow_right=unbind
keybind = super+arrow_up=unbind
keybind = super+arrow_down=unbind
keybind = super+shift+arrow_left=unbind
keybind = super+shift+arrow_right=unbind
keybind = super+shift+arrow_up=unbind
keybind = super+shift+arrow_down=unbind

# Keep Ghostty output copy and scrollback search on explicit terminal shortcuts.
keybind = ctrl+shift+c=copy_to_clipboard
keybind = ctrl+shift+f=start_search
```

Ghostty reads configuration from `~/.config/ghostty/config.ghostty` or `~/.config/ghostty/config`. On macOS it also reads `~/Library/Application Support/com.mitchellh.ghostty/config.ghostty` or `config`, with the macOS-specific file taking precedence. Create one of these files if you do not already have one.

Reload Ghostty with `Command+Shift+,` after saving the file. New and existing terminal surfaces then use the updated bindings.

## What changes globally

Ghostty configuration applies to every terminal surface, not only Carnet:

- `Shift+Arrow` is forwarded to terminal programs instead of extending Ghostty's terminal-output selection.
- `Command+C`, `Command+A`, `Command+Z`, `Command+Shift+Z`, and `Command+F` are forwarded to terminal programs.
- `Control+Shift+C` copies Ghostty's terminal-output selection.
- `Control+Shift+F` opens Ghostty's scrollback search.
- Mouse selection and Ghostty's copy-on-select behavior remain available.
- `Command+V` remains Ghostty's normal paste action. Carnet receives it as bracketed paste and records the whole paste as one undo step.
- `Option+Left/Right` retain Ghostty's shell-friendly legacy encoding; Carnet recognizes that encoding as word movement.

`Command+S`, `Command+P`, `Command+B`, and `Command+X` are not owned by Ghostty's default keymap and need no entries here. Ghostty maps `Command+Backspace` to `Control+U`; Carnet recognizes that input as delete-to-line-start.

## Validate the configuration

When Ghostty is installed in `/Applications`, validate the active configuration with:

```sh
/Applications/Ghostty.app/Contents/MacOS/ghostty +validate-config
```

The command exits silently when the configuration is valid and reports the exact invalid entry otherwise.
