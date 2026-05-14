use clap::Parser;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(name = "yeetdir", about = "フォルダ配下を並列で削除します")]
struct Cli {
    /// 削除対象のディレクトリ
    path: PathBuf,
    /// 中身が空になったあと、指定したディレクトリ自体も消す
    #[arg(short = 'r', long)]
    remove_root: bool,
    /// 使うスレッド数。省略時は CPU の並列度に合わせる
    #[arg(short = 'j', long)]
    jobs: Option<usize>,
    /// 進捗を出す
    #[arg(short = 'p', long, action = clap::ArgAction::SetTrue, conflicts_with = "no_progress")]
    progress: bool,
    /// 進捗を出さない
    #[arg(long, action = clap::ArgAction::SetTrue, conflicts_with = "progress")]
    no_progress: bool,
}

fn depth_key(path: &Path) -> usize {
    path.components().count()
}

fn style_spinner_walk() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.blue.bold} [{elapsed_precise}] {wide_msg} · {pos} 件")
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
}

fn style_bar(label: &'static str) -> ProgressStyle {
    ProgressStyle::with_template(&format!(
        "{{spinner:.green.bold}} [{{elapsed_precise}}] [{{wide_bar:.cyan/blue}}] {{human_pos}}/{{human_len}} {label} {{msg}}"
    ))
    .unwrap()
    .progress_chars("=>-")
}

fn collect_targets(
    root: &Path,
    walk_pb: Option<&ProgressBar>,
) -> std::io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut n = 0usize;

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
        if let Some(pb) = walk_pb {
            pb.inc(1);
            n += 1;
            if n.is_multiple_of(256) {
                pb.set_message(format!(
                    "ファイル {} · フォルダ {}",
                    files.len(),
                    dirs.len()
                ));
            }
        }
    }

    if let Some(pb) = walk_pb {
        pb.set_message(format!(
            "ファイル {} · フォルダ {}",
            files.len(),
            dirs.len()
        ));
    }

    Ok((files, dirs))
}

fn delete_files_parallel(
    files: &[PathBuf],
    pool: &rayon::ThreadPool,
    pb: Option<ProgressBar>,
) -> Vec<(PathBuf, std::io::Error)> {
    pool.install(|| {
        files
            .par_iter()
            .filter_map(|p| match std::fs::remove_file(p) {
                Ok(()) => {
                    if let Some(ref bar) = pb {
                        bar.inc(1);
                    }
                    None
                }
                Err(e) => Some((p.clone(), e)),
            })
            .collect()
    })
}

fn delete_dirs_by_depth(
    dirs: &[PathBuf],
    pool: &rayon::ThreadPool,
    pb: Option<ProgressBar>,
) -> Vec<(PathBuf, std::io::Error)> {
    let mut by_depth: BTreeMap<usize, Vec<PathBuf>> = BTreeMap::new();
    for d in dirs {
        by_depth.entry(depth_key(d)).or_default().push(d.clone());
    }

    let pb = pb.as_ref();
    let mut errors = Vec::new();
    for depth in by_depth.keys().rev() {
        let batch = by_depth.get(depth).unwrap();
        let batch_errs: Vec<_> = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|p| match std::fs::remove_dir(p) {
                    Ok(()) => {
                        if let Some(bar) = pb {
                            bar.inc(1);
                        }
                        None
                    }
                    Err(e) => Some((p.clone(), e)),
                })
                .collect()
        });
        errors.extend(batch_errs);
    }
    errors
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    let use_progress = !cli.no_progress && (cli.progress || std::io::stderr().is_terminal());

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

    let multi = use_progress.then(MultiProgress::new);

    let walk_pb = multi.as_ref().map(|m| {
        let pb = m.add(ProgressBar::new_spinner());
        pb.set_style(style_spinner_walk());
        pb.set_message("配下を読み取り中");
        pb.enable_steady_tick(Duration::from_millis(80));
        pb
    });

    let (files, dirs) = match collect_targets(root, walk_pb.as_ref()) {
        Ok(x) => x,
        Err(e) => {
            if let Some(pb) = &walk_pb {
                pb.abandon_with_message(format!("列挙に失敗: {e}"));
            }
            eprintln!("フォルダの列挙に失敗しました: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    if let Some(pb) = &walk_pb {
        pb.finish_with_message(format!(
            "列挙した（ファイル {} · フォルダ {}）",
            files.len(),
            dirs.len()
        ));
    }

    let file_pb = multi.as_ref().and_then(|m| {
        if files.is_empty() {
            return None;
        }
        let pb = m.add(ProgressBar::new(files.len() as u64));
        pb.set_style(style_bar("ファイル"));
        pb.set_message("削除中");
        Some(pb)
    });

    let mut file_errors = delete_files_parallel(&files, &pool, file_pb.clone());

    if let Some(pb) = file_pb {
        if file_errors.is_empty() {
            pb.finish_with_message("ファイルを削除しました");
        } else {
            pb.abandon_with_message(format!("ファイルで {} 件失敗", file_errors.len()));
        }
    }

    let dir_pb = multi.as_ref().and_then(|m| {
        if dirs.is_empty() {
            return None;
        }
        let pb = m.add(ProgressBar::new(dirs.len() as u64));
        pb.set_style(style_bar("フォルダ"));
        pb.set_message("削除中");
        Some(pb)
    });

    let mut dir_errors = delete_dirs_by_depth(&dirs, &pool, dir_pb.clone());

    if let Some(pb) = dir_pb {
        if dir_errors.is_empty() {
            pb.finish_with_message("フォルダを削除しました");
        } else {
            pb.abandon_with_message(format!("フォルダで {} 件失敗", dir_errors.len()));
        }
    }

    if cli.remove_root {
        match std::fs::remove_dir(root) {
            Ok(()) => {
                if let Some(m) = &multi {
                    let _ = m.println(format!("ルートを削除しました — {}", root.display()));
                }
            }
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

    if !fail && use_progress {
        if let Some(m) = &multi {
            let _ = m.println("完了");
        }
    }

    if fail {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    }
}
