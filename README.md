# Space Hog

Desktop disk-usage scanner in Rust (egui). Finds the largest files and folders and moves what you pick to the Trash.

## Download

Get the latest build from [Releases](https://github.com/maskeynihal/space-hog/releases):
`space-hog-vX.Y.Z-macos-universal.zip` (Apple Silicon and Intel) or `space-hog-vX.Y.Z-linux-x86_64.tar.gz`.

The macOS app is not notarized, so macOS blocks it the first time. After unzipping, run:

```sh
xattr -dr com.apple.quarantine "Space Hog.app"
```

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

## Releasing

Pushing to `main` creates a release only when a commit message in the push contains one of:

| Marker | Effect (from latest tag, e.g. v1.4.2) |
|---|---|
| `upgrade(major)` | v2.0.0 |
| `upgrade(minor)` | v1.5.0 |
| `upgrade(patch)` | v1.4.3 |
| `upgrade(vX.Y.Z)` | exactly vX.Y.Z (must be newer than the latest tag) |

An explicit version wins over a bump, and a bigger bump wins over a smaller one.
The [Release workflow](.github/workflows/release.yml) builds the macOS universal app and the Linux binary,
tags the commit, and publishes a GitHub release with generated notes.
