//! エクスプローラーの「フォルダを右クリック」メニュー（現在ユーザー向けレジストリ）と、
//! 複数選択時に起動された複数のプロセスを 1 回の削除にまとめる仕組み。
//!
//! 各プロセスはキューに自分のパスを置き、ロックを取れた 1 つ（リーダー）がまとめて処理する。
//! ロックは「ファイルを共有なしで開いていること」なので、リーダーが異常終了しても OS が解放する。

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const REG_PATH: &str = r"Software\Classes\Directory\shell\Yeetdir";
const MENU_LABEL: &str = "yeetdir で削除…";
/// これより古いキュー項目は、過去の操作の取り残しとして処理せずに捨てる。
const MAX_AGE: Duration = Duration::from_secs(60);

pub fn install() -> io::Result<()> {
    #[cfg(windows)]
    {
        install_windows()
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "この機能は Windows 専用です",
        ))
    }
}

pub fn uninstall() -> io::Result<()> {
    #[cfg(windows)]
    {
        uninstall_windows()
    }
    #[cfg(not(windows))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "この機能は Windows 専用です",
        ))
    }
}

#[cfg(windows)]
fn install_windows() -> io::Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(REG_PATH)?;
    key.set_value("", &MENU_LABEL)?;
    key.set_value("Icon", &exe_str.as_ref())?;
    key.set_value("MultiSelectModel", &"Player")?;

    let (cmd, _) = key.create_subkey("command")?;
    cmd.set_value("", &command_line(&exe_str))?;

    // 以前の版が作った PowerShell の仲介スクリプトは不要になった
    let _ = fs::remove_file(data_dir().join("context-menu-run.ps1"));
    Ok(())
}

#[cfg(windows)]
fn uninstall_windows() -> io::Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let registry = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(REG_PATH);
    registry.and(remove_own_files(&data_dir()))
}

/// yeetdir が作ったファイルだけを消し、空になったフォルダを片付ける。
/// 同じフォルダに置かれた exe などには触らない。
fn remove_own_files(data: &Path) -> io::Result<()> {
    let ok_if_missing = |r: io::Result<()>| match r {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    };
    let queue = data.join("context-menu-queue");
    if let Ok(rd) = fs::read_dir(&queue) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            let ours = p.extension().is_some_and(|x| x == "path" || x == "tmp")
                || p.file_name().is_some_and(|n| n == "leader.lock");
            if ours {
                ok_if_missing(fs::remove_file(&p))?;
            }
        }
    }
    ok_if_missing(fs::remove_file(data.join("context-menu-run.ps1")))?;
    // 空でなければ残す(ユーザーのファイルがある)
    let _ = fs::remove_dir(&queue);
    let _ = fs::remove_dir(data);
    Ok(())
}

/// レジストリに書くコマンド。exe のパスに `"` は入らず、`%1` は Explorer が展開する。
fn command_line(exe: &str) -> String {
    format!("\"{exe}\" context-menu-run \"%1\"")
}

fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("yeetdir")
}

/// 右クリックから起動されたときに呼ぶ。リーダーになったら、まとめた削除対象を返す。
/// 他のプロセスがまとめて処理するなら `None`。
pub fn collect_batch(path: &Path) -> io::Result<Option<Vec<PathBuf>>> {
    collect_batch_in(
        &data_dir().join("context-menu-queue"),
        path,
        Duration::from_millis(900),
    )
}

fn collect_batch_in(dir: &Path, path: &Path, settle: Duration) -> io::Result<Option<Vec<PathBuf>>> {
    fs::create_dir_all(dir)?;
    let item = enqueue(dir, path)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(_lock) = try_lock(dir)? {
            if !item.exists() {
                return Ok(None); // 前のリーダーが拾った
            }
            thread::sleep(settle); // 同時に起動された仲間がキューに置くのを待つ
            let mut paths = Vec::new();
            loop {
                let (fresh, stale) = drain(dir, SystemTime::now())?;
                if stale > 0 {
                    eprintln!(
                        "以前の操作で取り残された削除対象 {stale} 件は処理せずに捨てました。"
                    );
                }
                if fresh.is_empty() {
                    break;
                }
                paths.extend(fresh);
                thread::sleep(Duration::from_millis(300)); // 遅れて来た分も拾う
            }
            return Ok(Some(paths));
        }
        // フォロワー: 自分の項目がリーダーに拾われるのを待つ。
        // 拾われないまま前のリーダーが終わったら、次のループで自分がリーダーになる。
        thread::sleep(Duration::from_millis(100));
        if !item.exists() {
            return Ok(None);
        }
        if Instant::now() > deadline {
            // 消せたら誰にも拾われていない。消せなければ(NotFound)ちょうどリーダーが拾った
            return match fs::remove_file(&item) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                _ => Err(io::Error::other(
                    "ほかの yeetdir の処理待ちがタイムアウトしました",
                )),
            };
        }
    }
}

/// キューにパスを置く。一時名で書いてからリネームするので、書きかけを読まれない。
fn enqueue(dir: &Path, path: &Path) -> io::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let base = dir.join(format!("{nanos}-{}", std::process::id()));
    let tmp = base.with_extension("tmp");
    let item = base.with_extension("path");
    fs::write(&tmp, path_to_bytes(path))?;
    fs::rename(&tmp, &item)?;
    Ok(item)
}

/// キューの項目を全部取り出す。(新しい項目のパス, 古すぎて捨てた件数)。
/// 項目は「読んでから消せたプロセスのもの」。消す前に他が消していたら(NotFound)譲る。
/// 1 件の失敗で全体を失わないよう、項目ごとのエラーは警告して残す(古くなれば次回捨てる)。
fn drain(dir: &Path, now: SystemTime) -> io::Result<(Vec<PathBuf>, usize)> {
    let age = |modified: SystemTime| now.duration_since(modified).unwrap_or_default();
    let mut items = Vec::new();
    for entry in fs::read_dir(dir)?.filter_map(|e| e.ok()) {
        let p = entry.path();
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        match p.extension().and_then(|e| e.to_str()) {
            Some("path") => items.push((modified, p)),
            // 書いている途中で落ちたプロセスの残骸
            Some("tmp") if age(modified) > MAX_AGE => {
                let _ = fs::remove_file(&p);
            }
            _ => {}
        }
    }
    items.sort();

    let mut fresh = Vec::new();
    let mut stale = 0;
    for (modified, p) in items {
        let claimed = fs::read(&p).and_then(|bytes| fs::remove_file(&p).map(|()| bytes));
        match claimed {
            Ok(_) if age(modified) > MAX_AGE => stale += 1,
            Ok(bytes) => fresh.push(bytes_to_path(&bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("キューの項目を読めませんでした({}): {e}", p.display()),
        }
    }
    Ok((fresh, stale))
}

/// 取れたらロック(閉じると解放)を返す。ほかが持っていれば `None`。
fn try_lock(dir: &Path) -> io::Result<Option<fs::File>> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(0);
    }
    // ponytail: Windows 以外では共有モードがなくロックにならない。右クリック機能は Windows 専用なので未対応。
    match opts.open(dir.join("leader.lock")) {
        Ok(f) => Ok(Some(f)),
        Err(e) if e.raw_os_error() == Some(32) => Ok(None), // ERROR_SHARING_VIOLATION
        Err(e) => Err(e),
    }
}

#[cfg(windows)]
fn path_to_bytes(p: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn bytes_to_path(b: &[u8]) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let wide: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    std::ffi::OsString::from_wide(&wide).into()
}

#[cfg(not(windows))]
fn path_to_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(not(windows))]
fn bytes_to_path(b: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delete::tests::TempDir;

    #[test]
    fn command_line_quotes_exe_and_arg() {
        assert_eq!(
            command_line(r"C:\My Tools\yeetdir.exe"),
            r#""C:\My Tools\yeetdir.exe" context-menu-run "%1""#
        );
    }

    #[test]
    fn enqueue_drain_roundtrip_keeps_unicode_and_order() {
        let t = TempDir::new("queue");
        let a = PathBuf::from(r"C:\データ\it's [x] & y\");
        let b = PathBuf::from(r"D:\b");
        enqueue(&t.0, &a).unwrap();
        thread::sleep(Duration::from_millis(20));
        enqueue(&t.0, &b).unwrap();
        let (fresh, stale) = drain(&t.0, SystemTime::now()).unwrap();
        assert_eq!(fresh, vec![a, b]);
        assert_eq!(stale, 0);
        assert_eq!(fs::read_dir(&t.0).unwrap().count(), 0);
    }

    #[test]
    fn drain_discards_stale_items() {
        let t = TempDir::new("stale");
        enqueue(&t.0, Path::new(r"C:\old")).unwrap();
        let later = SystemTime::now() + MAX_AGE + Duration::from_secs(1);
        let (fresh, stale) = drain(&t.0, later).unwrap();
        assert!(fresh.is_empty());
        assert_eq!(stale, 1);
    }

    #[cfg(windows)]
    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let t = TempDir::new("lock");
        let first = try_lock(&t.0).unwrap();
        assert!(first.is_some());
        assert!(try_lock(&t.0).unwrap().is_none());
        drop(first);
        assert!(try_lock(&t.0).unwrap().is_some());
    }

    #[test]
    fn drain_removes_old_tmp_but_keeps_fresh_tmp() {
        let t = TempDir::new("tmp");
        fs::write(t.0.join("x.tmp"), "").unwrap();
        drain(&t.0, SystemTime::now()).unwrap();
        assert!(t.0.join("x.tmp").exists());
        drain(&t.0, SystemTime::now() + MAX_AGE + Duration::from_secs(1)).unwrap();
        assert!(!t.0.join("x.tmp").exists());
    }

    #[test]
    fn remove_own_files_keeps_foreign_files() {
        let t = TempDir::new("uninstall");
        let data = t.0.join("yeetdir");
        let queue = data.join("context-menu-queue");
        fs::create_dir_all(&queue).unwrap();
        for f in ["a.path", "b.tmp", "leader.lock"] {
            fs::write(queue.join(f), "").unwrap();
        }
        fs::write(data.join("context-menu-run.ps1"), "").unwrap();
        fs::write(data.join("yeetdir.exe"), "").unwrap();
        remove_own_files(&data).unwrap();
        assert!(!queue.exists());
        assert!(!data.join("context-menu-run.ps1").exists());
        assert!(data.join("yeetdir.exe").exists());

        fs::remove_file(data.join("yeetdir.exe")).unwrap();
        remove_own_files(&data).unwrap();
        assert!(!data.exists());
        remove_own_files(&data).unwrap(); // 何もなくても成功
    }

    #[test]
    fn single_invocation_becomes_leader() {
        let t = TempDir::new("leader");
        let got = collect_batch_in(&t.0, Path::new(r"C:\x"), Duration::ZERO).unwrap();
        assert_eq!(got, Some(vec![PathBuf::from(r"C:\x")]));
    }

    #[cfg(windows)]
    #[test]
    fn concurrent_invocations_deliver_each_path_exactly_once() {
        let t = TempDir::new("concurrent");
        let dir = t.0.clone();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.clone();
                thread::spawn(move || {
                    collect_batch_in(
                        &dir,
                        &PathBuf::from(format!(r"C:\p{i}")),
                        Duration::from_millis(50),
                    )
                    .unwrap()
                })
            })
            .collect();
        let mut all: Vec<PathBuf> = handles
            .into_iter()
            .filter_map(|h| h.join().unwrap())
            .flatten()
            .collect();
        all.sort();
        let mut want: Vec<PathBuf> = (0..8).map(|i| PathBuf::from(format!(r"C:\p{i}"))).collect();
        want.sort();
        assert_eq!(all, want);
        let left: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "path"))
            .collect();
        assert!(left.is_empty());
    }
}
