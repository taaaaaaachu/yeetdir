//! 右クリックの複数選択を模して `yeetdir context-menu-run` を同時に起動し、
//! 全パスがちょうど 1 回ずつ確認に出ること・何も消えないこと(入力がないので中止)を確かめる。
#![cfg(windows)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[test]
fn concurrent_processes_are_batched_without_loss() {
    let base = std::env::temp_dir().join(format!("yeetdir-it-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let appdata = base.join("appdata");
    fs::create_dir_all(&appdata).unwrap();
    let dirs: Vec<PathBuf> = (0..6)
        .map(|i| {
            let d = base.join(format!("target{i}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("f.txt"), "x").unwrap();
            d
        })
        .collect();

    let children: Vec<_> = dirs
        .iter()
        .map(|d| {
            Command::new(env!("CARGO_BIN_EXE_yeetdir"))
                .arg("context-menu-run")
                .arg(d)
                .env("LOCALAPPDATA", &appdata)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();

    let mut leaders = 0;
    let mut stderr = String::new();
    for c in children {
        let out = c.wait_with_output().unwrap();
        match out.status.code() {
            Some(0) => {}            // フォロワー
            Some(3) => leaders += 1, // リーダー(入力がないので中止)
            other => panic!(
                "想定外の終了コード {other:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        }
        stderr.push_str(&String::from_utf8_lossy(&out.stderr));
    }

    assert!(leaders >= 1);
    for d in &dirs {
        let shown = fs::canonicalize(d)
            .unwrap()
            .to_string_lossy()
            .replace(r"\\?\", "");
        let n = stderr.lines().filter(|l| l.trim() == shown).count();
        assert_eq!(n, 1, "{shown} が確認に出た回数\n{stderr}");
        assert!(d.join("f.txt").exists());
    }
    let queue = appdata.join("yeetdir").join("context-menu-queue");
    let left = fs::read_dir(&queue)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "path"))
        .count();
    assert_eq!(left, 0);
    let _ = fs::remove_dir_all(&base);
}
