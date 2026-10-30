//! Parallel filesystem scanner. Walks one filesystem, measures allocated
//! (on-disk) size, and aggregates per-directory totals.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jwalk::WalkDirGeneric;

pub const TOP_FILES: usize = 2000;

/// macOS keeps user data on a separate APFS volume that is "firmlinked" into `/`.
/// Scanning it directly counts everything once and matches `df` used space.
pub const MAC_DATA_VOLUME: &str = "/System/Volumes/Data";

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct DirStat {
    pub size: u64,
    pub files: u64,
}

pub struct ScanResult {
    pub root: PathBuf,
    pub total: u64,
    pub files: u64,
    pub errors: u64,
    pub elapsed: Duration,
    pub cancelled: bool,
    /// Largest files, biggest first.
    pub top_files: Vec<FileEntry>,
    /// Cumulative size of every directory (including all descendants).
    pub dirs: HashMap<PathBuf, DirStat>,
    /// Direct subdirectories of each directory.
    pub children: HashMap<PathBuf, Vec<PathBuf>>,
    /// (extension, bytes, count), biggest first.
    pub extensions: Vec<(String, u64, u64)>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Meta {
    alloc: u64,
    dev: u64,
    ino: u64,
    nlink: u64,
}

pub fn default_root() -> PathBuf {
    if cfg!(target_os = "macos") && Path::new(MAC_DATA_VOLUME).is_dir() {
        PathBuf::from(MAC_DATA_VOLUME)
    } else {
        PathBuf::from("/")
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Path as the user knows it (strips the macOS data-volume prefix).
pub fn pretty(path: &Path) -> String {
    match path.strip_prefix(MAC_DATA_VOLUME) {
        Ok(rest) => Path::new("/").join(rest).display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// Allocated bytes on disk, which is what actually fills the drive
/// (sparse files like Docker.raw report a much larger logical length).
fn allocated(md: &std::fs::Metadata) -> u64 {
    md.blocks() * 512
}

pub fn size_on_disk(path: &Path) -> u64 {
    std::fs::symlink_metadata(path).map(|m| allocated(&m)).unwrap_or(0)
}

pub fn scan(root: PathBuf, progress: Arc<Progress>) -> ScanResult {
    let started = Instant::now();
    let root_dev = std::fs::symlink_metadata(&root).map(|m| m.dev()).unwrap_or(0);

    let walk_progress = progress.clone();
    let walker = WalkDirGeneric::<((), Option<Meta>)>::new(&root)
        .skip_hidden(false)
        .follow_links(false)
        .sort(false)
        // Runs on rayon workers: stat in parallel and prune other filesystems here.
        .process_read_dir(move |_depth, dir_path, _state, children| {
            if walk_progress.cancel.load(Relaxed) {
                children.clear();
                return;
            }
            for entry in children.iter_mut().flatten() {
                match std::fs::symlink_metadata(dir_path.join(&entry.file_name)) {
                    Ok(md) => {
                        if md.dev() != root_dev {
                            entry.read_children = None;
                            continue;
                        }
                        entry.client_state = Some(Meta {
                            alloc: allocated(&md),
                            dev: md.dev(),
                            ino: md.ino(),
                            nlink: md.nlink(),
                        });
                    }
                    Err(_) => {
                        entry.read_children = None;
                        walk_progress.errors.fetch_add(1, Relaxed);
                    }
                }
            }
        });

    let mut own: HashMap<Arc<Path>, DirStat> = HashMap::new();
    own.insert(Arc::from(root.as_path()), DirStat::default());
    let mut heap: BinaryHeap<Reverse<(u64, PathBuf)>> = BinaryHeap::new();
    let mut seen_links: HashSet<(u64, u64)> = HashSet::new();
    let mut exts: HashMap<String, (u64, u64)> = HashMap::new();
    let mut total = 0u64;
    let mut files = 0u64;
    let mut cancelled = false;

    for item in walker {
        if progress.cancel.load(Relaxed) {
            cancelled = true;
            break;
        }
        let entry = match item {
            Ok(e) => e,
            Err(_) => {
                progress.errors.fetch_add(1, Relaxed);
                continue;
            }
        };
        if entry.depth == 0 {
            continue;
        }
        let Some(meta) = entry.client_state else { continue };

        if entry.file_type.is_dir() {
            own.entry(Arc::from(entry.path().as_path())).or_default();
            if progress.dirs.fetch_add(1, Relaxed) % 2048 == 0 {
                *progress.current.lock().unwrap() = entry.path();
            }
            continue;
        }

        // Count hard-linked files once.
        if meta.nlink > 1 && !seen_links.insert((meta.dev, meta.ino)) {
            continue;
        }
        let size = meta.alloc;
        total += size;
        files += 1;
        progress.files.fetch_add(1, Relaxed);
        progress.bytes.fetch_add(size, Relaxed);

        let stat = own.entry(entry.parent_path.clone()).or_default();
        stat.size += size;
        stat.files += 1;

        let ext = Path::new(&entry.file_name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "(none)".to_string());
        let e = exts.entry(ext).or_default();
        e.0 += size;
        e.1 += 1;

        if heap.len() < TOP_FILES {
            heap.push(Reverse((size, entry.path())));
        } else if heap.peek().is_some_and(|Reverse((min, _))| size > *min) {
            heap.pop();
            heap.push(Reverse((size, entry.path())));
        }
    }

    // Roll file sizes up the tree: deepest directories first.
    let mut order: Vec<(PathBuf, DirStat)> =
        own.into_iter().map(|(p, s)| (p.to_path_buf(), s)).collect();
    order.sort_unstable_by_key(|(p, _)| Reverse(p.components().count()));
    let mut dirs: HashMap<PathBuf, DirStat> = order.iter().cloned().collect();
    let mut children: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for (path, _) in &order {
        if *path == root {
            continue;
        }
        let Some(parent) = path.parent() else { continue };
        let stat = dirs[path];
        let p = dirs.entry(parent.to_path_buf()).or_default();
        p.size += stat.size;
        p.files += stat.files;
        children.entry(parent.to_path_buf()).or_default().push(path.clone());
    }

    let mut top_files: Vec<FileEntry> =
        heap.into_iter().map(|Reverse((size, path))| FileEntry { path, size }).collect();
    top_files.sort_unstable_by(|a, b| b.size.cmp(&a.size));

    let mut extensions: Vec<(String, u64, u64)> =
        exts.into_iter().map(|(k, (b, c))| (k, b, c)).collect();
    extensions.sort_unstable_by(|a, b| b.1.cmp(&a.1));

    ScanResult {
        root,
        total,
        files,
        errors: progress.errors.load(Relaxed),
        elapsed: started.elapsed(),
        cancelled,
        top_files,
        dirs,
        children,
        extensions,
    }
}

impl ScanResult {
    /// Map a user-facing path (e.g. ~/.npm) to the path used in this scan.
    pub fn resolve(&self, path: &Path) -> PathBuf {
        if self.root.starts_with(MAC_DATA_VOLUME) && !path.starts_with(MAC_DATA_VOLUME) {
            Path::new(MAC_DATA_VOLUME).join(path.strip_prefix("/").unwrap_or(path))
        } else {
            path.to_path_buf()
        }
    }

    pub fn size_of(&self, path: &Path) -> u64 {
        match self.dirs.get(path) {
            Some(d) => d.size,
            None => self
                .top_files
                .iter()
                .find(|f| f.path == path)
                .map(|f| f.size)
                .unwrap_or_else(|| size_on_disk(path)),
        }
    }

    /// Update totals after `path` (file or folder) was removed from disk.
    /// `size` must be measured before the delete (the path no longer exists).
    pub fn forget(&mut self, path: &Path, size: u64) {
        let files = self.dirs.get(path).map(|d| d.files).unwrap_or(1);
        let mut cur = path.parent();
        while let Some(dir) = cur {
            if let Some(d) = self.dirs.get_mut(dir) {
                d.size = d.size.saturating_sub(size);
                d.files = d.files.saturating_sub(files);
            }
            if dir == self.root {
                break;
            }
            cur = dir.parent();
        }
        if self.dirs.contains_key(path) {
            self.dirs.retain(|p, _| !p.starts_with(path));
            self.children.retain(|p, _| !p.starts_with(path));
        }
        if let Some(parent) = path.parent() {
            if let Some(kids) = self.children.get_mut(parent) {
                kids.retain(|k| k != path);
            }
        }
        self.top_files.retain(|f| !f.path.starts_with(path));
        self.total = self.total.saturating_sub(size);
        self.files = self.files.saturating_sub(files);
    }
}

/// Paths that must never be trashed from the UI.
pub fn is_protected(path: &Path, root: &Path) -> bool {
    if path == root || path.parent().is_none() {
        return true;
    }
    let shown = pretty(path);
    let p = Path::new(&shown);
    let home = home_dir();
    // Top-level folders that hold everything else; trashing one would wreck the system.
    let exact = [
        home.clone(),
        home.join("Library"),
        home.join("Library/Application Support"),
        home.join("Library/Containers"),
        PathBuf::from("/Applications"),
        PathBuf::from("/Users"),
        PathBuf::from("/Library"),
        PathBuf::from("/private"),
    ];
    if exact.iter().any(|e| e == p) {
        return true;
    }
    if p.starts_with("/usr/local") || p.starts_with("/opt") {
        return false;
    }
    ["/System", "/usr", "/bin", "/sbin", "/private/var/db", "/private/var/vm", "/Library/Apple"]
        .iter()
        .any(|bad| p.starts_with(bad))
}

pub struct DiskInfo {
    pub total: u64,
    /// Used across the whole drive (all APFS volumes), as System Settings shows it.
    pub used: u64,
    pub free: u64,
}

pub fn disk_info(path: &Path) -> Option<DiskInfo> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_bsize as u64;
    let total = s.f_blocks as u64 * block;
    let free = s.f_bavail as u64 * block;
    Some(DiskInfo { total, used: total.saturating_sub(free), free })
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}
