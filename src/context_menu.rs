//! エクスプローラーの「フォルダを右クリック」メニュー（現在ユーザー向けレジストリ）。

use std::io;

const REG_PATH: &str = r"Software\Classes\Directory\shell\Yeetdir";
const MENU_LABEL: &str = "yeetdir で削除…";

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
    use std::fs;
    use winreg::enums::*;
    use winreg::RegKey;

    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy();
    let script = context_menu_script_path();
    if let Some(parent) = script.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&script, context_menu_script(&exe_str))?;
    let script_str = script.to_string_lossy();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(REG_PATH)?;
    key.set_value("", &MENU_LABEL)?;
    key.set_value("Icon", &exe_str.as_ref())?;
    key.set_value("MultiSelectModel", &"Player")?;

    let (cmd, _) = key.create_subkey("command")?;
    let command_line = format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{script_str}\" \"%1\""
    );
    cmd.set_value("", &command_line)?;

    Ok(())
}

#[cfg(windows)]
fn uninstall_windows() -> io::Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let registry_result = hkcu.delete_subkey_all(REG_PATH);
    match std::fs::remove_file(context_menu_script_path()) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    registry_result
}

#[cfg(windows)]
fn context_menu_script_path() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("yeetdir")
        .join("context-menu-run.ps1")
}

#[cfg(windows)]
fn ps_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn context_menu_script(exe_path: &str) -> String {
    const TEMPLATE: &str = r#"
param(
    [Parameter(Mandatory = $true)]
    [string] $Path
)

$ErrorActionPreference = 'Stop'
$ExePath = {{EXE_PATH}}
$QueueDir = Join-Path $env:LOCALAPPDATA 'yeetdir\context-menu-queue'
New-Item -ItemType Directory -Force -Path $QueueDir | Out-Null

$ItemFile = Join-Path $QueueDir "$([guid]::NewGuid().ToString('N')).path"
[System.IO.File]::WriteAllText($ItemFile, $Path, [System.Text.Encoding]::UTF8)

$LockFile = Join-Path $QueueDir 'leader.lock'
$LockStream = $null
try {
    $LockStream = [System.IO.File]::Open(
        $LockFile,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
} catch {
    return
}

try {
    Start-Sleep -Milliseconds 900

    $Items = @(Get-ChildItem -LiteralPath $QueueDir -Filter '*.path' -File | Sort-Object CreationTimeUtc)
    $Paths = @()
    foreach ($Item in $Items) {
        $Text = Get-Content -LiteralPath $Item.FullName -Raw -Encoding UTF8
        if ($Text) {
            $Paths += $Text.TrimEnd("`r", "`n")
        }
    }
    foreach ($Item in $Items) {
        Remove-Item -LiteralPath $Item.FullName -Force -ErrorAction SilentlyContinue
    }

    if ($Paths.Count -eq 0) {
        return
    }

    $QuotedPaths = $Paths | ForEach-Object {
        '"' + ($_ -replace '"', '\"') + '"'
    }
    $ArgumentLine = '--remove-root ' + ($QuotedPaths -join ' ')
    Start-Process -FilePath $ExePath -ArgumentList $ArgumentLine
} finally {
    if ($null -ne $LockStream) {
        $LockStream.Close()
    }
    Remove-Item -LiteralPath $LockFile -Force -ErrorAction SilentlyContinue
}
"#;

    TEMPLATE
        .trim_start()
        .replace("{{EXE_PATH}}", &ps_single_quoted(exe_path))
}
