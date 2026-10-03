![Banner](docs/assets/banner.webp)

`moon` installs a downloaded archive, AppImage or `.deb` in one command. Zero crates, zero daemons, one binary. ( more features in future )

```
moon install app-1.2.3-linux-x64.tar.xz
```

It will:

1. <img src="docs/icons/FOLDER.svg" width="16" alt="" valign="-3"> extract the archive into `~/.local/share/moon/apps/<name>/`
2. <img src="docs/icons/LINK.svg" width="16" alt="" valign="-3"> symlink the executable(s) into `~/.local/bin` so you can run them from a terminal
3. <img src="docs/icons/LIST.svg" width="16" alt="" valign="-3"> write `~/.local/share/applications/<name>.desktop`, reusing the package's own `.desktop` file and icon when present
4. <img src="docs/icons/REFRESH.svg" width="16" alt="" valign="-3"> run `update-desktop-database` so it shows up in the applications menu
5. <img src="docs/icons/SAVE.svg" width="16" alt="" valign="-3"> remember everything in a manifest, so `moon remove` and `moon undo` can put it all back

## Commands

|  | command | what it does |
|:--:|---|---|
| <img src="docs/icons/PACKAGE.svg" width="16" alt="install"> | `moon install <file\|url>` | install or upgrade an app |
| <img src="docs/icons/SEARCH.svg" width="16" alt="picker"> | `moon` | interactive picker over the current directory |
| <img src="docs/icons/EXAM.svg" width="16" alt="inspect"> | `moon inspect <file\|url\|last>` | show what installing would do, change nothing |
| <img src="docs/icons/FILE.svg" width="16" alt="list"> | `moon list` | installed apps |
| <img src="docs/icons/TRASH.svg" width="16" alt="remove"> | `moon remove <name>...` | uninstall: binaries, menu entry, files |
| <img src="docs/icons/UNDO.svg" width="16" alt="undo"> | `moon undo [name]` | remove the app installed last (asks first) |
| <img src="docs/icons/WRENCH.svg" width="16" alt="doctor"> | `moon doctor [--fix]` | find broken entries and repair them |
| <img src="docs/icons/DRIVE.svg" width="16" alt="export"> | `moon export <name> [dir]` | copy an installed app as a portable bundle |
| <img src="docs/icons/TRASH.svg" width="16" alt="unexport"> | `moon unexport <name>` | delete a portable bundle |
| <img src="docs/icons/CLOCK.svg" width="16" alt="history"> | `moon history [-n N] [--clear]` | what was installed, and when |
| <img src="docs/icons/COG.svg" width="16" alt="config"> | `moon config` | settings |
| <img src="docs/icons/MOON.svg" width="16" alt="moon"> | `moon help`, `moon version` | |

Aliases where they feel natural: `ls` for list, `rm` for remove, `i`/`add` for install, `show`/`what` for inspect, `log`/`hist` for history, `check`/`verify`/`repair` for doctor.

### Install options

| option | meaning |
|---|---|
| `--name <name>` | app and command name (default: guessed from the file name) |
| `--bin <rel/path>` | the main executable inside the archive (default: auto-detected) |
| `--desktop` / `--no-desktop` | force or forbid a menu entry |
| `--force` | overwrite entries that moon does not own |
| `--portable-dir <path>` | keep the app on another disk instead of `~/.local` |

Re-running `install` on an installed app shows what changed and asks whether to upgrade, reinstall, keep or inspect first. In a script it just replaces, and says whether that was an upgrade, a reinstall or a downgrade.

Archives without an icon or `.desktop` file are treated as CLI tools: no menu entry unless you pass `--desktop`.

### <img src="docs/icons/PACKAGE.svg" width="18" alt="" valign="-4"> `.deb` packages

```
moon install nowly-host.deb --dry-run   # show the plan, write nothing
moon install nowly-host.deb             # install per-user into ~/.local
sudo moon install nowly-host.deb --root # dpkg-style, into the real /usr
```

| option | meaning |
|---|---|
| `--root` | install to the real `/usr`, `/opt`, ... (needs root) |
| `--run-scripts` | run the package's `preinst`/`postinst` (never by default) |
| `--no-rewrite` | don't rewrite absolute paths inside desktop files and wrappers |
| `--dry-run`, `-n` | show what would be installed |

Packages with files outside `/usr` (icons, appdata, desktop files) get relocated under `~/.local/share/moon/apps/<name>/system` and every absolute path inside the package is rewritten to match, so nothing ends up pointing at paths that don't exist. `--root` skips the relocation. Conflicts with files moon doesn't own are refused unless you pass `--force`.

### <img src="docs/icons/DRIVE.svg" width="18" alt="" valign="-4"> Portable installs

<img src="docs/icons/DRIVE.svg" width="16" alt="" valign="-4"> Keep an app on a USB stick instead of in `~/.local`:

```
moon install SomeApp.AppImage --portable-dir /mnt/USB/Apps
```

```
USB/
└── Apps/
    └── someapp/
        ├── SomeApp.AppImage
        ├── someapp.desktop
        └── icon.png
```

Command links in `~/.local/bin` and the menu entry still work; they just point into the bundle. Plug the drive into another machine and the app is there.

### <img src="docs/icons/SAVE.svg" width="18" alt="" valign="-4"> Exporting a bundle

<img src="docs/icons/SAVE.svg" width="16" alt="" valign="-4"> Turn anything moon already installed into a self-contained folder:

```
moon export someapp                 # creates ./someapp/
moon export someapp /mnt/USB/Apps   # or somewhere else
moon unexport someapp               # delete the bundle
```

The bundle carries the app, its menu entry and its icon, with absolute paths pointing inside the bundle. `moon doctor` flags a portable app whose directory has gone missing, which is what an unplugged USB stick looks like.

### <img src="docs/icons/WRENCH.svg" width="18" alt="" valign="-4"> doctor

```
moon doctor
```

```
Broken entries:

⚠ someapp 1.2.3
   Application missing: ~/.local/share/moon/apps/someapp
   Menu entry missing: ~/.local/share/applications/someapp.desktop

⚠ btop
   Symlink exists
   Target missing: ~/.local/bin/btop
   Points at: ~/.local/share/moon/apps/btop/bin/btop
```

<img src="docs/icons/WRENCH.svg" width="16" alt="" valign="-4"> `moon doctor --fix` removes the leftovers and updates the manifests. Add `--yes` to skip the question. It also finds command links in `~/.local/bin` that point at nothing and that moon did not create; those are reported but not touched.

## <img src="docs/icons/MAGIC.svg" width="18" alt="" valign="-4"> Icons

moon uses Nerd Font glyphs when it detects one and plain text when it does not, so the output stays readable either way:

```
moon config --icons auto   # default: icons only if a Nerd Font is installed
moon config --icons always
moon config --icons never
```

All 26 glyphs are single codepoints in the Font Awesome, Octicons and Devicons ranges of [Nerd Fonts](https://www.nerdfonts.com), so any patched Nerd Font covers them.

## Supported inputs

`.tar.gz` `.tar.xz` `.tar.bz2` `.tar.zst` `.tgz` `.tar` `.zip` `.7z` `.AppImage` `.deb`, bare executables, and `http(s)://` URLs.

`moon install` with no arguments opens a picker over the current directory: arrows or `hjkl` to move, `enter` to install, `q` to quit.

## <img src="docs/icons/FOLDER.svg" width="18" alt="" valign="-4"> Layout

```
src/
  main.rs        entry point
  cli.rs         argument parsing and dispatch
  config.rs      settings, Nerd Font detection, the icon set
  install.rs     install/upgrade, conflict prompts, remove, undo, list
  inspect.rs     the dry-run report
  doctor.rs      broken-entry detection and repair
  portable.rs    portable installs and bundles
  archive/       download and unpack
  deb/           ar, control file, install plan, rollback
  desktop.rs     .desktop and icon handling
  state.rs       manifests and last-install
  history.rs     install log
  tui/           terminal picker
  fsutil.rs      file and tree helpers
  naming.rs      guessing names, versions and architectures
  probe.rs       file sniffing, architecture, library checks
  paths.rs       XDG paths
  util.rs        text and size helpers
docs/
  icons/         one SVG per glyph, plus the generator that cuts them
```

## Uninstall moon

```
moon remove <name>... # the apps first
rm ~/.local/bin/moon
```
