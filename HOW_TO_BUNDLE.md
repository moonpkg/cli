# How to bundle

A `.moon` bundle is one file that holds a whole app: the program, its metadata,
its icon and its menu entry. No server, no account, no index. Copy the file to
the other machine and install it with one command.

    moon install SomeApp.moon

Under the hood a bundle is a plain `tar.gz`. Nothing about it is magic:

    tar tzf SomeApp.moon
    tar xzf SomeApp.moon  # yes, you can just unpack it

## The quickest way: bundle an app you already have installed

    moon install someapp-1.2.3-linux-x64.tar.xz
    moon bundle someapp  # writes ./someapp.moon

That is it. moon takes the files it installed, writes a manifest with paths
relative to the bundle, and packs everything into a single file. Give a path to
put it somewhere else, or `--force` to overwrite one that is already there:

    moon bundle someapp ~/Desktop/someapp.moon --force

## The other way: build the folder by hand

If you are shipping something you never installed through moon, make a folder
that looks like this:

    ytkew-moon/
    ├── ytkew.manifest
    ├── app/
    │   └── ytkew          / the program, plus whatever else it needs
    ├── desktop/
    │   └── ytkew.desktop  / optional, the menu entry
    └── icon/
        └── ytkew.svg      / optional, the icon

Then pack the folder:

    moon bundle ytkew-moon  # writes ytkew-moon.moon next to it

`tar -czf ytkew-moon.moon -C ytkew-moon .` produces the same file. moon only
reads the folder, it never needs to own it, so the folder can live in a build
script or a git repo.

## The manifest

One `.manifest` file at the top of the bundle, in the same `key=value` format
moon writes for everything it installs. Paths inside a bundle are relative to
the bundle root, never absolute:

    dir=app
    main=app/ytkew
    version=0.1.21
    desktop=desktop/ytkew.desktop
    link=ytkew
    to=app/ytkew
    cmd=ytkew

| key | what it means |
| --- | --- |
| `dir` | the folder holding the app, usually `app` |
| `main` | the program to run and to point the menu entry at |
| `version` | shown by `moon list`, and used when asking about upgrades |
| `desktop` | the bundled menu entry, if there is one |
| `link` / `to` | a command name to create in `~/.local/bin`, and what it points at |
| `cmd` | the command as the app itself spells it, for the menu entry |

Only `dir` and `main` really matter. Everything else is optional: with no
`link`/`to` pairs moon links the main program under the app's name, and with no
`desktop` it installs as a plain command line tool.

Repeat `link` and `to` in matching pairs for an app with more than one command:

    link=ffmpeg
    to=app/bin/ffmpeg
    link=ffprobe
    to=app/bin/ffprobe

## The menu entry and the icon

Keep the desktop entry in its own `desktop/` folder and the icon in `icon/`.
They travel inside the bundle and moon rewrites them on install, so the paths
in the file you ship do not have to be right for the machine it lands on.

Two rules worth knowing:

- `Icon=` should name the icon without a path, and the file should be named
  after it. `Icon=ytkew` resolves to `icon/ytkew.svg`. If you ship a `.png`
  with an `.svg` name, launchers will not find it.
- The `Exec=` line can be a bare command. moon replaces the program with the
  real path on the machine that installs the bundle and keeps the arguments
  (`%U`, `%F`, ...) as they are.

## Installing a bundle

    moon inspect SomeApp.moon               # what it would do, changes nothing
    moon install SomeApp.moon               # install it
    moon install SomeApp.moon --dry-run     # the plan, written nowhere
    moon install SomeApp.moon --name myapp  # install under another name

What happens:

1. the file is unpacked into `~/.local/share/moon/apps/<name>/`
2. the commands from `link`/`to` are symlinked into `~/.local/bin`
3. the bundled menu entry is rewritten and written to
   `~/.local/share/applications/<name>.desktop`, then
   `update-desktop-database` runs so the app shows up in your menu
4. the icon, if there is one, is copied next to the program
5. a manifest with this machine's real paths is saved, so `moon remove` and
   `moon undo` know what to clean up

## Things moon will not do

- Overwrite a command in `~/.local/bin` that moon did not create. Use
  `--force` if you mean it.
- Guess. If `main` is missing and no obvious program is there, moon stops and
  tells you to pass `--bin <path-in-app/>`.
- Run anything from the bundle. A bundle is files, never a script.

## Checking your work

    tar tzf SomeApp.moon       # does it unpack?
    moon inspect SomeApp.moon  # does moon understand it?

If `moon inspect` lists the commands, the files and the architecture, the
bundle is good. The architecture line is worth reading: a bundle built on one
machine will happily install on another, but only if the program inside was
built for it.