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
    use winreg::enums::*;
    use winreg::RegKey;

    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(REG_PATH)?;
    key.set_value("", &MENU_LABEL)?;
    key.set_value("Icon", &exe_str.as_ref())?;

    let (cmd, _) = key.create_subkey("command")?;
    let command_line = format!("\"{exe_str}\" \"%1\"");
    cmd.set_value("", &command_line)?;

    Ok(())
}

#[cfg(windows)]
fn uninstall_windows() -> io::Result<()> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.delete_subkey_all(REG_PATH)
}
