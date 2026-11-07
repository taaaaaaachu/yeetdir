use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

mod context_menu;
mod delete;
mod launch;

#[derive(Subcommand, Debug, Clone)]
enum MenuCmd {
    /// フォルダ右クリックに「yeetdir で削除…」を追加
    InstallContextMenu,
    /// 右クリックの登録を削除
    UninstallContextMenu,
    /// 右クリックメニューから呼ばれる（複数選択をまとめて 1 回で処理する）
    #[command(hide = true)]
    ContextMenuRun { path: PathBuf },
}

#[derive(Parser, Debug)]
#[command(
    name = "yeetdir",
    version,
    about = "フォルダの中身を並列で削除します（ゴミ箱には入りません）",
    args_conflicts_with_subcommands = true,
    after_help = "削除の前に対象を表示して確認します。確認なしで消すには --yes を付けてください。\n\
ジャンクション/シンボリックリンクはたどらず、リンク自体だけを消します。\n\
ドライブのルートやユーザーフォルダなどの保護対象と、その祖先は削除できません。\n\
\n\
Windows: エクスプローラーで yeetdir.exe にフォルダをドラッグすると、フォルダごと削除できます。\n\
コンテキストメニューへの登録: yeetdir install-context-menu で登録、uninstall-context-menu で解除。"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<MenuCmd>,
    #[command(flatten)]
    del: DeleteOptions,
}

#[derive(Args, Debug, Default)]
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
    /// 削除前の確認を省略する
    #[arg(short = 'y', long)]
    yes: bool,
    /// 失敗したとき、Enter キーが押されるまでウィンドウを閉じない
    #[arg(long)]
    pause: bool,
    /// 使うスレッド数。省略時は CPU の並列度に合わせる
    #[arg(short = 'j', long)]
    jobs: Option<usize>,
    /// 標準エラーがターミナル以外に渡されたときでも進捗を出す
    #[arg(short = 'p', long, conflicts_with = "no_progress")]
    progress: bool,
    /// 進捗を出さない
    #[arg(short = 'q', long, conflicts_with = "progress")]
    no_progress: bool,
}

const OK: u8 = 0;
const FAILED: u8 = 1;
const USAGE: u8 = 2;
const ABORTED: u8 = 3;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let from_explorer = launch::launched_from_explorer();

    let code = match cli.command {
        Some(MenuCmd::InstallContextMenu) => match context_menu::install() {
            Ok(()) => {
                eprintln!("登録しました。エクスプローラーでフォルダを右クリックしてください。");
                eprintln!(
                    "（Windows 11 では [その他のオプションを確認] の中に出ることがあります）"
                );
                eprintln!("実行ファイルを移動した際は、もう一度 install-context-menu を実行してください。");
                OK
            }
            Err(e) => {
                eprintln!("登録に失敗しました: {e}");
                FAILED
            }
        },
        Some(MenuCmd::UninstallContextMenu) => match context_menu::uninstall() {
            Ok(()) => {
                eprintln!("右クリックの登録を消しました。");
                OK
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("登録が見つかりませんでした（未登録か、すでに削除済み）。");
                OK
            }
            Err(e) => {
                eprintln!("登録の削除に失敗しました: {e}");
                FAILED
            }
        },
        Some(MenuCmd::ContextMenuRun { path }) => match context_menu::collect_batch(&path) {
            Ok(Some(paths)) => run_delete(
                &DeleteOptions {
                    paths,
                    remove_root: true,
                    pause: true,
                    ..Default::default()
                },
                false,
            ),
            Ok(None) => return std::process::ExitCode::SUCCESS, // 別のプロセスがまとめて処理する
            Err(e) => {
                eprintln!("右クリックからの起動に失敗しました: {e}");
                pause();
                FAILED
            }
        },
        None => run_delete(&cli.del, from_explorer),
    };
    std::process::ExitCode::from(code)
}

fn run_delete(del: &DeleteOptions, from_explorer: bool) -> u8 {
    let code = delete_flow(del, from_explorer);
    if (del.pause || from_explorer) && (code == FAILED || code == USAGE) {
        pause();
    }
    code
}

fn pause() {
    eprint!("Enter キーで閉じます…");
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}

fn delete_flow(del: &DeleteOptions, from_explorer: bool) -> u8 {
    if del.paths.is_empty() {
        eprintln!("削除するフォルダの PATH を 1 つ以上指定するか、サブコマンドを使ってください。");
        eprintln!("例: yeetdir install-context-menu");
        return USAGE;
    }

    let remove_root = del.remove_root || (from_explorer && !del.contents_only);

    let mut any_fail = false;
    let mut targets = Vec::new();
    for p in &del.paths {
        match delete::check_root(p) {
            Ok(c) => targets.push(c),
            Err(msg) => {
                eprintln!("{msg}: {}", p.display());
                any_fail = true;
            }
        }
    }
    if targets.is_empty() {
        return FAILED;
    }

    if !del.yes && !confirm(&targets, remove_root) {
        eprintln!("中止しました。");
        return ABORTED;
    }

    let threads = del
        .jobs
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(4)
        .max(1);
    // 再帰で深く潜るので、スタックは大きめにとる(予約だけで実メモリは使わない)
    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .stack_size(16 << 20)
        .build()
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("スレッドプールを起動できませんでした: {e}");
            return FAILED;
        }
    };

    let use_progress = !del.no_progress && (del.progress || std::io::stderr().is_terminal());

    for root in &targets {
        let shown = delete::display(root);
        // 確認を待つ間にリンクへ差し替えられていないか、消す直前にもう一度見る
        if delete::check_root(root).as_ref() != Ok(root) {
            eprintln!("確認のあとでフォルダが変わったため、削除しませんでした: {shown}");
            any_fail = true;
            continue;
        }
        let pb = use_progress.then(|| {
            // 全体の件数は、削除と並行して数えながら増えていく
            let pb = ProgressBar::new(0);
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green.bold} [{elapsed_precise}] [{bar:30.cyan/blue}] 削除 {human_pos} / {human_len} 件 {wide_msg}",
                )
                .unwrap()
                .progress_chars("=>-"),
            );
            if !std::io::stderr().is_terminal() {
                // 既定の出力先は端末でないと描画しないので、--progress のときは無条件に描く出力先にする
                pb.set_draw_target(ProgressDrawTarget::term_like(Box::new(
                    console::Term::stderr(),
                )));
            }
            pb.set_message(shown.clone());
            pb.enable_steady_tick(Duration::from_millis(80));
            delete::Progress::new(pb)
        });

        let errors = delete::Errors::default();
        let stop = AtomicBool::new(false);
        let mut ok = std::thread::scope(|s| {
            if let Some(p) = &pb {
                s.spawn(|| delete::count_tree(root, p, &stop));
            }
            let ok = pool.install(|| delete::remove_contents(root, pb.as_ref(), &errors));
            stop.store(true, Ordering::Relaxed);
            ok
        });
        let mut errors = errors.into_inner().unwrap();
        if ok && remove_root {
            if let Err(e) = delete::remove_dir(root) {
                errors.push((root.clone(), e));
                ok = false;
            }
        }

        if let Some(pb) = pb.map(|p| p.bar) {
            if ok {
                pb.set_length(pb.position()); // 見積もりを確定値にする
                pb.finish_with_message(format!("{shown} — 削除しました"));
            } else {
                pb.abandon_with_message(format!("{shown} — {} 件失敗", errors.len()));
            }
        }
        for (p, e) in &errors {
            eprintln!("削除できませんでした — {}\n    {e}", delete::display(p));
        }
        any_fail |= !ok;
    }

    if any_fail {
        FAILED
    } else {
        if use_progress {
            eprintln!("完了");
        }
        OK
    }
}

/// 対象を表示して y/N を聞く。入力できない環境では安全側に倒して中止する。
fn confirm(targets: &[PathBuf], remove_root: bool) -> bool {
    let what = if remove_root {
        "フォルダごと"
    } else {
        "中身だけ"
    };
    eprintln!("次のフォルダを{what}削除します。ゴミ箱には入りません。");
    for t in targets {
        eprintln!("  {}", delete::display(t));
    }
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        eprintln!(
            "確認の入力ができないため中止します。確認なしで消すには --yes を付けてください。"
        );
        return false;
    }
    eprint!("よろしいですか? [y/N]: ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    stdin.lock().read_line(&mut answer).is_ok() && is_yes(&answer)
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delete::tests::TempDir;

    #[test]
    fn is_yes_accepts_only_explicit_yes() {
        assert!(is_yes("y\r\n"));
        assert!(is_yes(" YES "));
        assert!(!is_yes(""));
        assert!(!is_yes("n"));
        assert!(!is_yes("yeah"));
    }

    #[test]
    fn delete_flow_with_yes_removes_root_or_contents() {
        let t = TempDir::new("flow");
        let a = t.0.join("a");
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::write(a.join("sub/f"), "x").unwrap();
        let opts = |remove_root| DeleteOptions {
            paths: vec![a.clone()],
            remove_root,
            yes: true,
            no_progress: true,
            ..Default::default()
        };
        assert_eq!(delete_flow(&opts(false), false), OK);
        assert!(a.exists());
        assert_eq!(std::fs::read_dir(&a).unwrap().count(), 0);
        assert_eq!(delete_flow(&opts(true), false), OK);
        assert!(!a.exists());
    }

    #[test]
    fn delete_flow_fails_for_missing_path_and_empty_args() {
        let t = TempDir::new("flow-missing");
        let opts = DeleteOptions {
            paths: vec![t.0.join("nope")],
            yes: true,
            no_progress: true,
            ..Default::default()
        };
        assert_eq!(delete_flow(&opts, false), FAILED);
        assert_eq!(delete_flow(&DeleteOptions::default(), false), USAGE);
    }

    #[cfg(windows)]
    #[test]
    fn delete_flow_refuses_junction_root_and_keeps_target() {
        let t = TempDir::new("flow-junc");
        let target = t.0.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "x").unwrap();
        let st = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(t.0.join("link"))
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(st.success());
        let opts = DeleteOptions {
            paths: vec![t.0.join("link")],
            remove_root: true,
            yes: true,
            no_progress: true,
            ..Default::default()
        };
        assert_eq!(delete_flow(&opts, false), FAILED);
        assert!(target.join("keep.txt").exists());
        assert!(t.0.join("link").exists());
    }

    #[cfg(windows)]
    #[test]
    fn delete_flow_reports_failure_when_root_cannot_be_removed() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let t = TempDir::new("flow-busy");
        let a = t.0.join("busy");
        std::fs::create_dir(&a).unwrap();
        // 削除を許さない共有モードでフォルダを開いたままにする
        let _hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2) // READ | WRITE(DELETE なし)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&a)
            .unwrap();
        let opts = DeleteOptions {
            paths: vec![a.clone()],
            remove_root: true,
            yes: true,
            no_progress: true,
            ..Default::default()
        };
        assert_eq!(delete_flow(&opts, false), FAILED);
        assert!(a.exists());
    }

    #[test]
    fn cli_parses_flags_and_hidden_subcommand() {
        let cli = Cli::try_parse_from(["yeetdir", "-y", "-r", "--pause", "C:\\x"]).unwrap();
        assert!(cli.del.yes && cli.del.remove_root && cli.del.pause);
        let cli = Cli::try_parse_from(["yeetdir", "context-menu-run", "C:\\x"]).unwrap();
        assert!(matches!(cli.command, Some(MenuCmd::ContextMenuRun { .. })));
        assert!(Cli::try_parse_from(["yeetdir", "-r", "--contents-only", "C:\\x"]).is_err());
    }

    #[test]
    fn version_flag_prints_package_version() {
        for flag in ["--version", "-V"] {
            let err = Cli::try_parse_from(["yeetdir", flag]).unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
            assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
        }
    }
}
