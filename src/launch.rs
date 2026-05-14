//! Windows で親プロセスが explorer.exe かどうか（ドラッグ＆ドロップ等）を見る。

/// エクスプローラー経由で起動されたら `true`。それ以外（cmd / PowerShell / IDE 端末など）は `false`。
pub fn launched_from_explorer() -> bool {
    #[cfg(windows)]
    {
        launched_from_explorer_windows()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn launched_from_explorer_windows() -> bool {
    use std::ffi::OsStr;
    use std::mem;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let my_pid = unsafe { GetCurrentProcessId() };

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return false;
    }

    let parent_pid = unsafe {
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snap, &mut entry) == 0 {
            CloseHandle(snap);
            return false;
        }

        let mut parent = None;
        loop {
            if entry.th32ProcessID == my_pid {
                parent = Some(entry.th32ParentProcessID);
                break;
            }
            if Process32NextW(snap, &mut entry) == 0 {
                break;
            }
        }
        CloseHandle(snap);
        parent
    };

    let Some(parent_pid) = parent_pid else {
        return false;
    };

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return false;
    }

    let name = unsafe {
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;

        let mut found = None;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                if entry.th32ProcessID == parent_pid {
                    let end = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    found = Some(std::ffi::OsString::from_wide(&entry.szExeFile[..end]));
                    break;
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        found
    };

    name.as_ref()
        .map(|n| n.as_os_str().eq_ignore_ascii_case(OsStr::new("explorer.exe")))
        .unwrap_or(false)
}
