mod app;
mod scanner;

use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--cli") {
        let root = args.get(2).map(PathBuf::from).unwrap_or_else(scanner::default_root);
        run_cli(root);
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Space Hog")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    // `space-hog <folder>` opens the window and scans that folder right away.
    let initial_root = args.get(1).filter(|a| !a.starts_with('-')).cloned();
    eframe::run_native(
        "Space Hog",
        options,
        Box::new(|_cc| Ok(Box::new(app::App::new(initial_root)))),
    )
}

fn run_cli(root: PathBuf) {
    use scanner::{human, pretty};
    eprintln!("Scanning {} ...", root.display());
    let res = scanner::scan(root.clone(), Arc::new(scanner::Progress::default()));
    println!(
        "Total {} in {} files, {} unreadable, {:.1}s",
        human(res.total),
        res.files,
        res.errors,
        res.elapsed.as_secs_f64()
    );
    if let Some(d) = scanner::disk_info(&root) {
        println!(
            "Disk: {} used of {} ({} free)",
            human(d.used),
            human(d.total),
            human(d.free)
        );
    }
    println!("\nLargest files:");
    for f in res.top_files.iter().take(20) {
        println!("  {:>10}  {}", human(f.size), pretty(&f.path));
    }
    println!("\nLargest folders directly under {}:", pretty(&root));
    let mut kids = res.children.get(&root).cloned().unwrap_or_default();
    kids.sort_by_key(|k| std::cmp::Reverse(res.dirs[k].size));
    for k in kids.iter().take(20) {
        println!("  {:>10}  {}", human(res.dirs[k].size), pretty(k));
    }
    println!("\nLargest file types:");
    for (ext, bytes, count) in res.extensions.iter().take(10) {
        println!("  {:>10}  .{ext} ({count} files)", human(*bytes));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "macos")]
    fn trash_moves_file_without_prompt() {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let name = format!("space-hog-test-{}.txt", std::process::id());
        let path = std::env::temp_dir().join(&name);
        std::fs::write(&path, b"hello").unwrap();
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn protects_top_level_folders() {
        let home = crate::scanner::home_dir();
        let root = std::path::Path::new("/");
        assert!(crate::scanner::is_protected(&home.join("Library"), root));
        assert!(crate::scanner::is_protected(std::path::Path::new("/System/Library"), root));
        assert!(!crate::scanner::is_protected(&home.join("Library/Caches"), root));
        assert!(!crate::scanner::is_protected(&home.join(".npm"), root));
    }
}
