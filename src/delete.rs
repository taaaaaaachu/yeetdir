//! リンクをたどらない並列削除と、削除してはいけないパスの判定。

use indicatif::ProgressBar;
use rayon::prelude::*;
use std::fs::{self, FileType};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

pub type Errors = Mutex<Vec<(PathBuf, io::Error)>>;

/// 進捗バーと、全体の件数の 2 つの見積もり。
/// 全体の件数は「削除側が見つけた数」と「数える専用スレッドの数」の大きい方にする
/// (足すと二重に数えてしまうため)。
pub struct Progress {
    pub bar: ProgressBar,
    found: AtomicU64,
    counted: AtomicU64,
}

impl Progress {
    pub fn new(bar: ProgressBar) -> Self {
        Progress {
            bar,
            found: AtomicU64::new(0),
            counted: AtomicU64::new(0),
        }
    }

    fn found(&self, n: u64) {
        self.found.fetch_add(n, Ordering::Relaxed);
        self.update_length();
    }

    fn update_length(&self) {
        let len = self
            .found
            .load(Ordering::Relaxed)
            .max(self.counted.load(Ordering::Relaxed));
        // 別スレッドが小さい値で上書きしても、次の更新で直る
        if self.bar.length().is_none_or(|l| l < len) {
            self.bar.set_length(len);
        }
    }
}

/// `root` の下を件数だけ数える(削除と並行して走らせる)。`stop` が立ったら途中でやめる。
/// 幅優先なので、深さ優先で進む削除側より先に、浅い所から全体を見渡せる。
/// 削除側が先に消したフォルダは読めないが、その分は削除側が見つけているので問題ない。
pub fn count_tree(root: &Path, progress: &Progress, stop: &AtomicBool) {
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut n = 0;
        for e in rd.filter_map(|e| e.ok()) {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            n += 1;
            if e.file_type().is_ok_and(|ft| ft.is_dir()) {
                queue.push_back(e.path());
            }
        }
        progress.counted.fetch_add(n, Ordering::Relaxed);
        progress.update_length();
    }
}

/// 削除対象として指定されたパスを検査し、正規化した絶対パス(`\\?\` 形式)を返す。
/// リンク・ファイル・保護対象のフォルダ(とその祖先)は拒否する。
pub fn check_root(root: &Path) -> Result<PathBuf, String> {
    let meta = fs::symlink_metadata(root).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => "そのパスはありません".to_string(),
        _ => format!("パスを確認できませんでした({e})"),
    })?;
    if meta.file_type().is_symlink() {
        return Err("リンク(ジャンクション/シンボリックリンク)は対象外です".into());
    }
    if !meta.is_dir() {
        return Err("フォルダを指定してください".into());
    }
    let canon = fs::canonicalize(root).map_err(|e| format!("パスを解決できませんでした({e})"))?;
    let (exact, subtree) = protected_dirs();
    if let Some(p) = protected_hit(&canon, &exact, &subtree) {
        return Err(format!(
            "保護対象のフォルダ({})を含むため削除できません",
            display(&p)
        ));
    }
    Ok(canon)
}

/// 次のどれかに当たれば、当たった保護対象を返す。
/// - ドライブ/共有のルート
/// - `exact` のどれかそのもの、またはその祖先
/// - `subtree` のどれかそのもの、その祖先、またはその子孫
pub fn protected_hit(target: &Path, exact: &[PathBuf], subtree: &[PathBuf]) -> Option<PathBuf> {
    if target.parent().is_none() {
        return Some(target.to_path_buf());
    }
    exact
        .iter()
        .chain(subtree)
        .find(|p| p.starts_with(target))
        .or_else(|| subtree.iter().find(|p| target.starts_with(p)))
        .cloned()
}

/// (祖先まで保護するもの, 子孫まで保護するもの)。環境変数から作る。
// ponytail: Known Folder は既定の名前で決め打ち。D: などへリダイレクトしている環境では
// SHGetKnownFolderPath で引くようにする。
fn protected_dirs() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut exact: Vec<PathBuf> = [
        "USERPROFILE",
        "HOME",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
        "APPDATA",
        "LOCALAPPDATA",
        "OneDrive",
        "PUBLIC",
    ]
    .iter()
    .filter_map(|k| env(k))
    .collect();
    if let Some(home) = env("USERPROFILE") {
        for name in [
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
        ] {
            exact.push(home.join(name));
        }
        // ほかのユーザーのプロファイル(Public / Default も含む)
        if let Some(Ok(rd)) = home.parent().map(fs::read_dir) {
            exact.extend(rd.filter_map(|e| e.ok()).map(|e| e.path()));
        }
    }
    let subtree: Vec<PathBuf> = ["SystemRoot", "windir"]
        .iter()
        .filter_map(|k| env(k))
        .collect();
    if cfg!(windows) && (subtree.is_empty() || env("USERPROFILE").is_none()) {
        eprintln!("警告: SystemRoot / USERPROFILE が取れないため、保護対象の判定が不完全です。");
    }
    let canon = |v: Vec<PathBuf>| {
        v.into_iter()
            .filter_map(|p| fs::canonicalize(p).ok())
            .collect()
    };
    (canon(exact), canon(subtree))
}

/// 表示用に `\\?\` / `\\?\UNC\` を外す。
pub fn display(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    }
}

/// `dir` の中身を並列で消す(`dir` 自体は残す)。全部消せたら `true`。
/// リンクはたどらず、リンク自体だけを消す。失敗は `errors` に積む。
/// 子が消せなかったフォルダは削除を試みない(「空ではありません」の連鎖を出さないため)。
// ponytail: パス基準の削除なので、列挙後に途中のフォルダをリンクへ差し替えられると木の外を消せる(TOCTOU)。
// 他人が書き込めるフォルダを管理者権限で消す用途が出たら、ハンドル相対の削除にする。
pub fn remove_contents(dir: &Path, pb: Option<&Progress>, errors: &Errors) -> bool {
    let entries = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            push(errors, dir, e);
            return false;
        }
    };
    let mut ok = true;
    let mut items = Vec::new();
    for entry in entries {
        match entry.and_then(|e| Ok((e.path(), e.file_type()?))) {
            Ok(x) => items.push(x),
            Err(e) => {
                push(errors, dir, e);
                ok = false;
            }
        }
    }
    // 見つけた分を全体の件数に反映する
    if let Some(pb) = pb {
        pb.found(items.len() as u64);
    }
    let all = items
        .par_iter()
        .map(|(path, ft)| remove_entry(path, *ft, pb, errors))
        .reduce(|| true, |a, b| a && b);
    ok && all
}

/// 空のフォルダ(リンクではないこと)を消す。一時的な失敗はリトライし、
/// 読み取り専用属性で拒否されたら属性を外してやり直す。
pub fn remove_dir(path: &Path) -> io::Result<()> {
    retry(|| clear_readonly_on_denied(path, || fs::remove_dir(path)))
}

fn remove_entry(path: &Path, ft: FileType, pb: Option<&Progress>, errors: &Errors) -> bool {
    // FileType はリンクをたどらないので、リンクの is_dir() は false になる
    let result = if ft.is_dir() {
        if !remove_contents(path, pb, errors) {
            return false;
        }
        remove_dir(path)
    } else {
        retry(|| remove_link_or_file(path, ft))
    };
    match result {
        Ok(()) => {
            if let Some(pb) = pb {
                pb.bar.inc(1);
            }
            true
        }
        Err(e) => {
            push(errors, path, e);
            false
        }
    }
}

fn remove_link_or_file(path: &Path, ft: FileType) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        // フォルダへのシンボリックリンクとジャンクションは remove_dir でリンクだけ消える
        if ft.is_symlink_dir() {
            return fs::remove_dir(path);
        }
    }
    if ft.is_symlink() {
        return fs::remove_file(path);
    }
    // リンクではない普通のファイルだけ、属性を外してよい(リンクだとリンク先の属性を変えてしまう)
    clear_readonly_on_denied(path, || fs::remove_file(path))
}

/// アクセス拒否のとき、読み取り専用属性が付いていれば外して 1 回だけやり直す。
/// (新しい std は読み取り専用でも消せるが、古い std 向けの保険)
fn clear_readonly_on_denied(path: &Path, f: impl Fn() -> io::Result<()>) -> io::Result<()> {
    match f() {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let Ok(meta) = fs::symlink_metadata(path) else {
                return Err(e);
            };
            let mut perm = meta.permissions();
            if meta.file_type().is_symlink() || !perm.readonly() {
                return Err(e);
            }
            #[allow(clippy::permissions_set_readonly_false)]
            // Windows の読み取り専用属性を外すだけ
            perm.set_readonly(false);
            fs::set_permissions(path, perm)?;
            f()
        }
        r => r,
    }
}

/// 共有違反と「空でない」(削除保留の残り)は少し待って 3 回まで、アクセス拒否は 1 回だけやり直す。
/// 既に無いものは成功扱い。
// ponytail: 握っているプロセス名は出さない。必要になったら Restart Manager API で調べる。
fn retry(mut f: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    let mut wait = 50;
    let mut denied_retried = false;
    loop {
        match f() {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e)
                if wait <= 200
                    && (e.kind() == io::ErrorKind::DirectoryNotEmpty
                        || e.raw_os_error() == Some(ERROR_SHARING_VIOLATION)) => {}
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && !denied_retried => {
                denied_retried = true;
            }
            r => return r,
        }
        std::thread::sleep(Duration::from_millis(wait));
        wait *= 2;
    }
}

fn push(errors: &Errors, path: &Path, e: io::Error) {
    errors.lock().unwrap().push((path.to_path_buf(), e));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// テスト用の一時フォルダ(drop で消える)。
    pub(crate) struct TempDir(pub PathBuf);
    impl TempDir {
        pub(crate) fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "yeetdir-test-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(fs::canonicalize(&p).unwrap())
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run(dir: &Path) -> (bool, Vec<(PathBuf, io::Error)>) {
        let errors = Errors::default();
        let ok = remove_contents(dir, None, &errors);
        (ok, errors.into_inner().unwrap())
    }

    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) {
        let st = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(st.success());
    }

    #[test]
    fn removes_nested_tree_and_keeps_root() {
        let t = TempDir::new("nested");
        fs::create_dir_all(t.0.join("a/b/c")).unwrap();
        fs::write(t.0.join("a/b/c/f.txt"), "x").unwrap();
        fs::write(t.0.join("top.txt"), "x").unwrap();
        let (ok, errs) = run(&t.0);
        assert!(ok, "{errs:?}");
        assert!(t.0.exists());
        assert_eq!(fs::read_dir(&t.0).unwrap().count(), 0);
    }

    #[test]
    fn progress_length_grows_with_discovered_entries() {
        let t = TempDir::new("progress");
        fs::create_dir_all(t.0.join("a/b")).unwrap();
        fs::write(t.0.join("a/b/f1"), "x").unwrap();
        fs::write(t.0.join("a/f2"), "x").unwrap();
        fs::write(t.0.join("f3"), "x").unwrap();
        let hidden = || {
            Progress::new(ProgressBar::with_draw_target(
                Some(0),
                indicatif::ProgressDrawTarget::hidden(),
            ))
        };

        // 数える専用スレッドは、消さずに全体(a, a/b, a/b/f1, a/f2, f3)を数える
        let counted = hidden();
        count_tree(&t.0, &counted, &AtomicBool::new(false));
        assert_eq!(counted.bar.length(), Some(5));

        // 停止の合図が立っていれば何も数えない
        let stopped = hidden();
        count_tree(&t.0, &stopped, &AtomicBool::new(true));
        assert_eq!(stopped.bar.length(), Some(0));

        // 削除側が見つけた数と大きい方が全体になる(二重に数えない)
        let pb = hidden();
        pb.counted.store(5, Ordering::Relaxed);
        let errors = Errors::default();
        assert!(remove_contents(&t.0, Some(&pb), &errors));
        assert_eq!(pb.bar.length(), Some(5));
        assert_eq!(pb.bar.position(), 5);
    }

    #[test]
    fn removes_readonly_file() {
        let t = TempDir::new("ro");
        let f = t.0.join("ro.txt");
        fs::write(&f, "x").unwrap();
        let mut perm = fs::metadata(&f).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&f, perm).unwrap();
        let (ok, errs) = run(&t.0);
        assert!(ok, "{errs:?}");
        assert!(!f.exists());
    }

    #[cfg(windows)]
    #[test]
    fn removes_trailing_dot_name_via_canonical_root() {
        let t = TempDir::new("dot");
        let root = check_root(&t.0).unwrap();
        fs::write(root.join("foo."), "x").unwrap();
        let (ok, errs) = run(&root);
        assert!(ok, "{errs:?}");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn junction_inside_tree_removes_only_link() {
        let t = TempDir::new("junc-in");
        let outside = t.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "x").unwrap();
        let root = t.0.join("root");
        fs::create_dir(&root).unwrap();
        junction(&root.join("link"), &outside);
        let (ok, errs) = run(&root);
        assert!(ok, "{errs:?}");
        assert!(!root.join("link").exists());
        assert!(outside.join("keep.txt").exists());
    }

    #[cfg(windows)]
    #[test]
    fn check_root_rejects_junction() {
        let t = TempDir::new("junc-root");
        let target = t.0.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), "x").unwrap();
        junction(&t.0.join("link"), &target);
        assert!(check_root(&t.0.join("link"))
            .unwrap_err()
            .contains("リンク"));
        assert!(target.join("keep.txt").exists());
    }

    #[test]
    fn check_root_rejects_missing_and_file() {
        let t = TempDir::new("check");
        assert!(check_root(&t.0.join("nope")).is_err());
        fs::write(t.0.join("f"), "x").unwrap();
        assert!(check_root(&t.0.join("f")).unwrap_err().contains("フォルダ"));
        assert!(check_root(&t.0).is_ok());
    }

    #[test]
    fn protected_hit_blocks_roots_ancestors_and_system_subtree() {
        let home = PathBuf::from(r"C:\Users\me");
        let win = PathBuf::from(r"C:\Windows");
        let (ex, sub) = (vec![home.clone()], vec![win.clone()]);
        let hit = |p: &str| protected_hit(Path::new(p), &ex, &sub);
        assert_eq!(hit(r"C:\"), Some(PathBuf::from(r"C:\")));
        assert_eq!(hit(r"C:\Users"), Some(home.clone()));
        assert_eq!(hit(r"C:\Users\me"), Some(home.clone()));
        assert_eq!(hit(r"C:\Users\me\work"), None);
        assert_eq!(hit(r"C:\Users\meow"), None);
        assert_eq!(hit(r"C:\Windows"), Some(win.clone()));
        assert_eq!(hit(r"C:\Windows\System32"), Some(win.clone()));
        assert_eq!(hit(r"C:\WindowsApps"), None);
    }

    #[cfg(windows)]
    #[test]
    fn real_protected_dirs_are_rejected() {
        for k in ["USERPROFILE", "SystemRoot"] {
            let p = PathBuf::from(std::env::var_os(k).unwrap());
            assert!(check_root(&p).is_err(), "{k}");
        }
        let sys32 = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        assert!(check_root(&sys32).is_err());
        let docs = PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("Documents");
        if docs.exists() {
            assert!(check_root(&docs).is_err());
        }
    }

    #[test]
    fn removes_readonly_dir() {
        let t = TempDir::new("rodir");
        let d = t.0.join("ro");
        fs::create_dir(&d).unwrap();
        let mut perm = fs::metadata(&d).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&d, perm).unwrap();
        let (ok, errs) = run(&t.0);
        assert!(ok, "{errs:?}");
        assert!(!d.exists());
    }

    #[cfg(windows)]
    #[test]
    fn removes_tree_deeper_than_max_path() {
        let t = TempDir::new("long");
        let root = check_root(&t.0).unwrap();
        let mut deep = root.clone();
        for _ in 0..30 {
            deep.push("a_fairly_long_directory_name");
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("f.txt"), "x").unwrap();
        assert!(deep.as_os_str().len() > 800);
        let (ok, errs) = run(&root);
        assert!(ok, "{errs:?}");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn unreadable_subdir_is_reported_and_parent_kept() {
        let t = TempDir::new("denied");
        let sub = t.0.join("locked");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("f"), "x").unwrap();
        let user = std::env::var("USERNAME").unwrap();
        let icacls = |args: &[&str]| {
            std::process::Command::new("icacls")
                .arg(&sub)
                .args(args)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap()
        };
        assert!(icacls(&["/deny", &format!("{user}:(RD)")]).success());
        let (ok, errs) = run(&t.0);
        icacls(&["/remove:d", &user]);
        assert!(!ok);
        assert!(errs.iter().any(|(p, _)| p == &sub), "{errs:?}");
        assert!(sub.join("f").exists());
    }

    #[test]
    fn display_strips_verbatim_prefix() {
        assert_eq!(display(Path::new(r"\\?\C:\a\b")), r"C:\a\b");
        assert_eq!(display(Path::new(r"\\?\UNC\srv\share\x")), r"\\srv\share\x");
        assert_eq!(display(Path::new(r"C:\a")), r"C:\a");
    }

    #[test]
    fn retry_treats_not_found_as_success_and_retries_transient() {
        assert!(retry(|| Err(io::ErrorKind::NotFound.into())).is_ok());
        let mut n = 0;
        let r = retry(|| {
            n += 1;
            if n < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(())
            }
        });
        assert!(r.is_ok());
        assert_eq!(n, 3);
        let mut m = 0;
        let r = retry(|| {
            m += 1;
            Err(io::ErrorKind::InvalidInput.into())
        });
        assert!(r.is_err());
        assert_eq!(m, 1);
    }
}
