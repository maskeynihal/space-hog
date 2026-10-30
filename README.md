# Space Hog

Desktop disk-usage scanner in Rust (egui). Finds the largest files and folders and moves what you pick to the Trash.

## Build

```sh
./bundle.sh            # builds "Space Hog.app"
```

Move `Space Hog.app` to `/Applications`, then grant it **Full Disk Access**
(System Settings → Privacy & Security → Full Disk Access) so whole-disk scans can read protected folders.
Rebuilding re-signs the app, so you need to grant access again after each rebuild.

## Use

- `Scan` with `/` scans the whole disk; `~` scans your home folder.
- **Largest files**: top 2000 files. Filter by path, tick several files, move them to the Trash together.
- **Folders**: drill down by size, like a tree map in list form.
- **Cleanup ideas**: known caches (npm, Xcode, Docker, pnpm, ...) and every project `node_modules`.
- **File types**: space by extension. Click a type to list its files.

Trashing frees space only after you empty the Trash. Trash is disabled for system and top-level folders.

## Command line

```sh
target/release/space-hog --cli ~      # print the top files and folders
target/release/space-hog ~            # open the window and scan right away
```

Sizes are allocated bytes on disk (sparse files such as `Docker.raw` count only what they really use), and hard links count once.
