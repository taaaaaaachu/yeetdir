use clap::Parser;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(name = "yeetdir", about = "フォルダ配下を並列で削除します")]
struct Cli {
    /// 消す対象のディレクトリ（配下をまとめて削除）
    path: PathBuf,
    /// 中身が空になったあと、指定したディレクトリ自体も消す
    #[arg(long)]
    remove_root: bool,
    /// 使うスレッド数。省略時は CPU の並列度に合わせる
    #[arg(short = 'j', long)]
    jobs: Option<usize>,
}

fn depth_key(path: &Path) -> usize {
    path.components().count()
}

fn collect_targets(root: &Path) -> std::io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();

    for entry in WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path().to_path_buf();
        if entry.file_type().is_dir() {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }

    Ok((files, dirs))
}

fn delete_files_parallel(files: &[PathBuf], pool: &rayon::ThreadPool) -> Vec<(PathBuf, std::io::Error)> {
    pool.install(|| {
        files
            .par_iter()
            .filter_map(|p| std::fs::remove_file(p).err().map(|e| (p.clone(), e)))
            .collect()
    })
}

fn delete_dirs_by_depth(dirs: &[PathBuf], pool: &rayon::ThreadPool) -> Vec<(PathBuf, std::io::Error)> {
    let mut by_depth: BTreeMap<usize, Vec<PathBuf>> = BTreeMap::new();
    for d in dirs {
        by_depth.entry(depth_key(d)).or_default().push(d.clone());
    }

    let mut errors = Vec::new();
    for depth in by_depth.keys().rev() {
        let batch = by_depth.get(depth).unwrap();
        let batch_errs: Vec<_> = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|p| std::fs::remove_dir(p).err().map(|e| (p.clone(), e)))
                .collect()
        });
        errors.extend(batch_errs);
    }
    errors
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    let root = &cli.path;
    if !root.exists() {
        eprintln!("そのパスはありません: {}", root.display());
        return std::process::ExitCode::from(1);
    }
    if !root.is_dir() {
        eprintln!("フォルダを指定してください: {}", root.display());
        return std::process::ExitCode::from(1);
    }

    let threads = cli
        .jobs
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(4)
        .max(1);

    let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("スレッドプールを起動できませんでした: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    let (files, dirs) = match collect_targets(root) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("フォルダの列挙に失敗しました: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    let mut file_errors = delete_files_parallel(&files, &pool);
    let mut dir_errors = delete_dirs_by_depth(&dirs, &pool);

    if cli.remove_root {
        match std::fs::remove_dir(root) {
            Ok(()) => {}
            Err(e) => dir_errors.push((root.clone(), e)),
        }
    }

    let fail = !file_errors.is_empty() || !dir_errors.is_empty();
    for (p, e) in file_errors.drain(..) {
        eprintln!("ファイルを消せませんでした — {}\n    {e}", p.display());
    }
    for (p, e) in dir_errors.drain(..) {
        eprintln!("フォルダを消せませんでした — {}\n    {e}", p.display());
    }

    if fail {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    }
}
