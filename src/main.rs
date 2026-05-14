use clap::{Args, Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;
use walkdir::WalkDir;

mod context_menu;
mod launch;

#[derive(Subcommand, Debug, Clone)]
enum MenuCmd {
    /// フォルダ右クリックに「yeetdir で削除…」を追加（現在の exe を現ユーザーに登録）
    InstallContextMenu,
    /// 右クリックの登録を削除
    UninstallContextMenu,
}

#[derive(Parser, Debug)]
#[command(
    name = "yeetdir",
    about = "フォルダ配下を並列で削除します",
    args_conflicts_with_subcommands = true,
    after_help = "Windows: エクスプローラーにフォルダを yeetdir.exe へドラッグした起動では、親が explorer.exe のときフォルダ本体も消します。cmd / PowerShell などからは従来どおり中身だけです（本体も消すときは -r）。中身だけにしたいドラッグには --contents-only。\n\
\n\
右クリック: yeetdir install-context-menu で登録、uninstall-context-menu で解除。"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<MenuCmd>,
    #[command(flatten)]
    del: DeleteOptions,
}

#[derive(Args, Debug)]
struct DeleteOptions {
    /// 削除対象のディレクトリ（複数可）
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,
    /// 中身が空になったあと、指定したディレクトリ自体も消す
    #[arg(short = 'r', long, conflicts_with = "contents_only")]
    remove_root: bool,
    /// エクスプローラー経由の起動でも、中身だけ消してルートフォルダは残す
    #[arg(long, conflicts_with = "remove_root")]
    contents_only: bool,
    /// 使うスレッド数。省略時は CPU の並列度に合わせる
    #[arg(short = 'j', long)]
    jobs: Option<usize>,
    /// 標準エラーが端末じゃないときでも進捗を出す（既定は端末なら出る）
    #[arg(short = 'p', long, action = clap::ArgAction::SetTrue, conflicts_with = "no_progress")]
    progress: bool,
    /// 進捗を出さない（端末でもオフにしたいとき）
    #[arg(short = 'q', long, action = clap::ArgAction::SetTrue, conflicts_with = "progress")]
    no_progress: bool,
}

enum RunOutcome {
    EnumerateFailed {
        root: PathBuf,
        error: std::io::Error,
    },
    Done {
        file_errors: Vec<(PathBuf, std::io::Error)>,
        dir_errors: Vec<(PathBuf, std::io::Error)>,
    },
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

fn run_delete_one(
    root: &Path,
    remove_root: bool,
    pool: &rayon::ThreadPool,
    multi: Option<&MultiProgress>,
) -> RunOutcome {
    let walk_pb = multi.map(|m| {
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
            return RunOutcome::EnumerateFailed {
                root: root.to_path_buf(),
                error: e,
            };
        }
    };

    if let Some(pb) = &walk_pb {
        pb.finish_with_message(format!(
            "削除対象（ファイル {} · フォルダ {}）",
            files.len(),
            dirs.len()
        ));
    }

    let file_pb = multi.and_then(|m| {
        if files.is_empty() {
            return None;
        }
        let pb = m.add(ProgressBar::new(files.len() as u64));
        pb.set_style(style_bar("ファイル"));
        pb.set_message("削除中");
        Some(pb)
    });

    let file_errors = delete_files_parallel(&files, &pool, file_pb.clone());

    if let Some(pb) = file_pb {
        if file_errors.is_empty() {
            pb.finish_with_message("ファイルを削除しました");
        } else {
            pb.abandon_with_message(format!("ファイルで {} 件失敗", file_errors.len()));
        }
    }

    let dir_pb = multi.and_then(|m| {
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

    if remove_root {
        match std::fs::remove_dir(root) {
            Ok(()) => {
                if let Some(m) = multi {
                    let _ = m.println(format!("ルートを削除しました — {}", root.display()));
                }
            }
            Err(e) => dir_errors.push((root.to_path_buf(), e)),
        }
    }

    RunOutcome::Done {
        file_errors,
        dir_errors,
    }
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Some(MenuCmd::InstallContextMenu) => {
            return match context_menu::install() {
                Ok(()) => {
                    eprintln!("登録しました。エクスプローラーでフォルダを右クリックしてください。");
                    eprintln!("（Windows 11 では [さらに表示] の中に出ることがあります）");
                    eprintln!("exe を移したら、もう一度 install-context-menu を実行してください。");
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("登録に失敗しました: {e}");
                    std::process::ExitCode::from(1)
                }
            };
        }
        Some(MenuCmd::UninstallContextMenu) => {
            return match context_menu::uninstall() {
                Ok(()) => {
                    eprintln!("右クリックの登録を消しました。");
                    std::process::ExitCode::SUCCESS
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("登録が見つかりませんでした（未登録か、すでに削除済み）。");
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("登録の削除に失敗しました: {e}");
                    std::process::ExitCode::from(1)
                }
            };
        }
        None => {}
    }

    if cli.del.paths.is_empty() {
        eprintln!("削除するフォルダの PATH を 1 つ以上指定するか、サブコマンドを使ってください。");
        eprintln!("例: yeetdir install-context-menu");
        return std::process::ExitCode::from(2);
    }

    let use_progress = !cli.del.no_progress
        && (cli.del.progress || std::io::stderr().is_terminal());

    let threads = cli
        .del
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

    let from_explorer = launch::launched_from_explorer();
    let remove_root = cli.del.remove_root || (from_explorer && !cli.del.contents_only);

    let mut any_fail = false;

    for root in &cli.del.paths {
        if !root.exists() {
            eprintln!("そのパスはありません: {}", root.display());
            any_fail = true;
            continue;
        }
        if !root.is_dir() {
            eprintln!("フォルダを指定してください: {}", root.display());
            any_fail = true;
            continue;
        }

        if let Some(m) = &multi {
            let _ = m.println(format!("── {}", root.display()));
        }

        match run_delete_one(
            root,
            remove_root,
            &pool,
            multi.as_ref(),
        ) {
            RunOutcome::EnumerateFailed { root, error } => {
                eprintln!("フォルダの列挙に失敗しました — {}\n    {error}", root.display());
                any_fail = true;
            }
            RunOutcome::Done {
                mut file_errors,
                mut dir_errors,
            } => {
                any_fail |= !file_errors.is_empty() || !dir_errors.is_empty();
                for (p, e) in file_errors.drain(..) {
                    eprintln!("ファイルを消せませんでした — {}\n    {e}", p.display());
                }
                for (p, e) in dir_errors.drain(..) {
                    eprintln!("フォルダを消せませんでした — {}\n    {e}", p.display());
                }
            }
        }
    }

    if !any_fail && use_progress {
        if let Some(m) = &multi {
            let _ = m.println("完了");
        }
    }

    if any_fail {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    }
}
