# Keybinds

Everything in the main view is remappable in `~/.config/vorgan/config.toml`
(an action name can take one key or a list). The Graph screen keys are fixed.
The same actions are in the toolbar and the right-click menu.

## Main view

| Key | Action |
|---|---|
| `j` `k`, arrows | Cursor down / up |
| `l` `right` `enter` | Open: dive into a folder, browse an archive, open a file with `xdg-open` |
| `h` `left` `backspace` | Go to the parent folder |
| `g` `home` / `G` `end` | Top / bottom of the list |
| `space` | Toggle selection on the item under the cursor |
| `ctrl+a` | Select everything visible |
| `y` | Copy selection to the internal clipboard |
| `x` | Cut selection |
| `p` | Paste into the current folder |
| `r`, `f2` | Rename |
| `a`, `f10` | New folder |
| `A` | New file |
| `d`, `delete` | Move to trash |
| `D`, `shift+delete` | Delete permanently (asks first) |
| `e` | Extract: selected archive, or the archive you are browsing (password prompt if needed) |
| `z` | Compress: name, format, destination, optional zip password |
| `s` | Cycle sort: name → size → date → type |
| `.` | Show / hide hidden files |
| `/` | Live filter; type to narrow, `enter` to leave to the list, `escape` clears |
| `F` | Search every subfolder of the current one for a name, from a prompt (results in a menu) |
| `u` | Undo the last trash or the last move (facts from the undo journal) |
| `f` | Hint tags: each visible row gets a tag, type it to jump (double letters when the list is long) |
| `m` | Pin the current folder to a letter |
| `'` | Jump ("warp") to the folder pinned on a letter |
| `~` | Frequent folders, from your fish history |
| `tab` | Open the Graph |
| `t` | Open a terminal in this folder (`$TERMINAL`, default `foot`) |
| `ctrl+r` | Refresh |
| `q` | Quit and drop the shell into the folder you ended in (via the `vg` fish function) |
| `Q`, `ctrl+q` | Quit, shell stays put |
| `?` | Help overlay |
| `escape` | Clear selection, or clear the filter |

## Graph screen (Tab)

| Key | Action |
|---|---|
| `Click a bubble` | Dive into it (the graph recentres on its folders) |
| `Click the centre bubble` | Close the graph and go to that folder |
| `Click the dim up-arrow` | Go up one level |
| `Drag a bubble near the centre` | Pin it there (gold dotted ring; remembered) |
| `Right-click a pinned bubble` | Unpin |
| `Mouse wheel` / `+` `-` | Zoom the graph |
| `tab`, `esc` | Close the graph and go here |
| `enter` | Dive into the selected bubble |
| `q` | Cancel, stay in the folder the graph opened from |
| `j` `k`, arrows | Cycle the bubbles by keyboard |
| `l` `right` `space` | Dive into the selected bubble |
| `h` `backspace` `left` | Up one level |
| `.` | Toggle hidden folders |
| `n` | Next page when a folder has more subfolders than fit |
| `r` | Reset the layout (pinned bubbles stay pinned) |
| `0` | Reset zoom |

## Mouse

| Do this | What happens |
|---|---|
| Wheel | Scrolls the pane under the pointer (list, trail or preview) two rows per notch; the cursor stays put |
| Click | Select (double-click opens: folder dive or `xdg-open`) |
| Ctrl-click | Add / remove from selection |
| Shift-click | Select a range |
| Drag onto a folder | Move (hold `ctrl` to copy) |
| Right-click | Context menu: open, clipboard, rename, trash, extract, compress, copy path, terminal, properties |
| Click the breadcrumb | Jump to that folder |
| Click the toolbar | The same actions as the keys |
| Drag a pane edge | Resize the trails and preview (remembered across runs) |

## Remapping

```toml
# ~/.config/vorgan/config.toml
[keys]
graph = "g"             # one key or a list: graph = ["g", "f2"]
rename = ["r", "F"]
```

Remappable action names: `down up enter back top bottom graph hint search find hidden pin warp toggle
selall copy cut paste rename newdir newfile trash delete extract compress undo quit_cd quit help sort
refresh frecent terminal clear`.
