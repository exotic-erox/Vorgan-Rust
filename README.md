# Vorgan (Broken Rn fixing...)


A mouse-and-keyboard file manager for the terminal — now as a single Rust
binary. Its signature move is the **Graph** (press `Tab`): your current folder,
its folders and its archives as bubbles you click through like Obsidian's graph
view, with a physics layout, pinning and zoom. Everything from the python
version is here — same keys, same config, same state files — plus a few things
that only the native port can do:

- **Sixel image previews** — pictures fill the preview pane at the terminal's
  real pixel size, damage-tracked so they never flicker
- **Deep search** `F` — find files by name across every subfolder, open from
  the results menu
- **Undo** `u` — brings back the last trashed or moved item through an undo
  journal
- **Progress bars** — big copies, trashes, extracts and searches show a bar in
  the status line as they run
- A smoother graph: the bubbles drift with the particle system from
  [vizr](https://pypi.org/project/vizr/)
- One ~10 MB binary, no python, no pip

![Vorgan](preview.png)

The Graph:

![The Graph](graph-preview.png)

Made on Arch + Hyprland + fish with the Caelestia palette, but it runs in any
terminal with mouse support, and it reads its colours live from whatever rice
you point it at.

## Install

### Arch

The easiest: clone this repo and let makepkg do the work. It asks for `cargo`
and `libarchive`, builds the release binary, runs the tests and installs
`/usr/bin/vorgan`:

```bash
git clone https://github.com/exotic-erox/vorgan
cd vorgan
makepkg -si
```

If the package is published on the AUR as `vorgan`, `paru -S vorgan` or
`yay -S vorgan` does the same. The `PKGBUILD` in the repo root is that package.

### Any other distro (cargo)

1. Install a Rust toolchain — [rustup.rs](https://rustup.rs) or
   `sudo apt install rustc cargo` on Debian/Ubuntu,
   `sudo dnf install rust cargo` on Fedora — plus the libarchive binary
   (`bsdtar`, used for archive folders):
   - Debian / Ubuntu: `sudo apt install libarchive-tools`
   - Fedora: `sudo dnf install bsdtar`
   - Arch: `sudo pacman -S --needed libarchive`

2. Clone and install to `~/.cargo/bin`:

   ```bash
   git clone https://github.com/exotic-erox/vorgan
   cd vorgan
   cargo install --path .
   ```

   Make sure `~/.cargo/bin` is on your `PATH` (rustup does this for you).

3. Run it: `vorgan ~/Documents`, or `vorgan` inside a folder.

### Quit and cd

`q` quits and is meant to drop your **shell** into the folder you ended in —
the terminal itself cannot do that, so the launcher function does. Fish users
copy it in once:

```bash
mkdir -p ~/.config/fish/functions
cp shell/vg.fish ~/.config/fish/functions/vg.fish
```

and run `vg` from then on. For bash/zsh, put this in your `~/.bashrc` or
`~/.zshrc`:

```bash
vorgan() {
    local tmp="$(mktemp)"
    command vorgan --cwd-file="$tmp" "$@"
    if [ -s "$tmp" ]; then
        local dir; read -r dir < "$tmp"
        [ -d "$dir" ] && [ "$dir" != "$PWD" ] && cd "$dir"
    fi
    command rm -f -- "$tmp"
}
```

`Q` (or `ctrl+q`) quits without moving the shell.

## Files, previews, the Graph and everything else

Everything works with the mouse (click, double-click, ctrl/shift click, drag
onto a folder to move, right-click menus, toolbar) and with the keyboard. The
full keybind list lives in [docs/KEYBINDS.md](docs/KEYBINDS.md); this is the
daily bread:

| Key | Action |
|---|---|
| `j k h l` / arrows | Move, open, up |
| `f` | Hint tags: type the tag to jump to any visible item |
| `m` + letter, `'` + letter | Pin the current folder, jump back to it |
| `/` | Live filter, `.` hidden files, `s` sort |
| `space`, `ctrl+a` | Select |
| `y x p` | Copy, cut, paste |
| `r`, `a`, `A`, `d`, `D` | Rename, new folder, new file, trash, delete forever |
| `e`, `z` | Extract (here or to a path), compress (name, format, path, optional zip password) |
| `F` | Search every subfolder below you for a name |
| `u` | Undo the last trash / move |
| `~` | Frequent folders from your fish history |
| drag a pane edge | Resize the three panes (click to save; remembered across runs) |
| `t` | Open a terminal here |
| `q` / `Q` | Quit and cd / quit |

### Image previews

Select a picture and the preview pane paints it as sixels — the terminal's
true pixels, not half-block approximations. Adding files or scrolling reuses
the drawn image until the pane or the file actually changes. Needs a sixel
terminal (foot 1.20+ is what it is developed against); in anything else you
get text and metadata previews instead.

### The Graph (Tab)

| Do this | What happens |
|---|---|
| Click a bubble | Dive into that folder. The graph re-centres and shows its folders. |
| Click the centre bubble | Close the graph and go to that folder. |
| Click the dim up-arrow bubble | Go up one level. |
| Drag a bubble close to the centre | Pin it there (gold dotted ring). Pins are remembered. |
| Right-click a pinned bubble | Unpin it. |
| Mouse wheel / `+` `-` | Zoom. |
| Hover a bubble | Highlights its links, shows the names of its folders. |
| `+N more` bubble | Next page when a folder has many subfolders. |

Zip, tar, 7z and other archives show up as purple bubbles you can dive into
like folders. The bubbles never sit still entirely — they breathe with the
vizr particles, and physics goes to sleep when the graph settles.

## Configuration

`~/.config/vorgan/config.toml` (optional):

```toml
icons = "nerd"          # or "plain" if your font has no Nerd glyphs
show_hidden = false
sort = "name"           # name | size | date | type
archive_format = "zip"  # zip | tar.gz | tar.zst | tar.xz | 7z
graph_max_nodes = 28

[keys]
graph = "g"             # remap any action; see docs/KEYBINDS.md for all names
```

## Files it uses

| Path | Purpose |
|---|---|
| `~/.config/vorgan/config.toml` | Settings and key remaps (optional) |
| `~/.local/state/vorgan/` | Graph pins (`pins.json`), letter anchors (`anchors.json`), pane widths (`cols.json`), undo journal (`undo.json`) |
| `~/.local/state/caelestia/scheme.json` | Read-only: colours, re-read live (see docs/THEME.md) |

These are the same paths the python version reads — your pins, anchors and
column widths carry over untouched. No scheme file means the built-in dark
palette.

## Requirements

Rust to build, then at runtime: `libarchive` (bsdtar), a terminal with mouse
support (foot is what it is developed against), a Nerd Font for icons
(`icons = "plain"` otherwise). Optional: `wl-clipboard` (copy path),
`xdg-utils` (open files), and a sixel-capable terminal for image previews.

## Development

```
cargo test
cargo fmt
```

More docs: [KEYBINDS](docs/KEYBINDS.md) · [THEME](docs/THEME.md)

## License

MIT — see [LICENSE](LICENSE).

## Social

[![PyPI](https://img.shields.io/badge/PyPI-vizr-3775A9?style=for-the-badge&logo=pypi&logoColor=white)](https://pypi.org/project/vizr/)
[![GitHub](https://img.shields.io/badge/GitHub-vizr-181717?style=for-the-badge&logo=github&logoColor=white)](https://github.com/exotic-erox/vizr)
[![Discord](https://img.shields.io/badge/Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white)](https://discord.com/users/1515183303995560096)
