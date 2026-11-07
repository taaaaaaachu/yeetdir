//! Windows で親プロセスが explorer.exe かどうか（ドラッグ＆ドロップ等）を見る。

use std::ffi::OsString;

/// エクスプローラー経由で起動されたら `true`。それ以外（cmd / PowerShell / IDE 端末など）は `false`。
/// 削除範囲を変える判定なので、呼び出し側は必ず削除前に確認を挟むこと。
pub fn launched_from_explorer() -> bool {
    #[cfg(windows)]
    {
        parent_is_explorer(&process_snapshot(), std::process::id())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// (pid, 親 pid, exe 名) の一覧から、`me` の親が explorer.exe かを判定する。
// ponytail: 親が終了して PID が再利用されていると誤判定しうる。削除前の確認で人が見るので許容。
fn parent_is_explorer(procs: &[(u32, u32, OsString)], me: u32) -> bool {
    let Some(&(_, parent, _)) = procs.iter().find(|p| p.0 == me) else {
        return false;
    };
    procs
        .iter()
        .find(|p| p.0 == parent)
        .is_some_and(|p| p.2.eq_ignore_ascii_case("explorer.exe"))
}

#[cfg(windows)]
fn process_snapshot() -> Vec<(u32, u32, OsString)> {
    use std::mem;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut procs = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return procs;
        }
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            let end = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            procs.push((
                entry.th32ProcessID,
                entry.th32ParentProcessID,
                OsString::from_wide(&entry.szExeFile[..end]),
            ));
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    procs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, name: &str) -> (u32, u32, OsString) {
        (pid, ppid, name.into())
    }

    #[test]
    fn detects_explorer_parent() {
        let procs = [p(1, 0, "Explorer.EXE"), p(2, 1, "yeetdir.exe")];
        assert!(parent_is_explorer(&procs, 2));
    }

    #[test]
    fn other_or_missing_parent_is_false() {
        let procs = [
            p(1, 0, "pwsh.exe"),
            p(2, 1, "yeetdir.exe"),
            p(3, 99, "yeetdir.exe"),
        ];
        assert!(!parent_is_explorer(&procs, 2));
        assert!(!parent_is_explorer(&procs, 3));
        assert!(!parent_is_explorer(&procs, 42));
    }

    #[cfg(windows)]
    #[test]
    fn snapshot_contains_self() {
        let me = std::process::id();
        assert!(process_snapshot().iter().any(|p| p.0 == me));
    }
}
