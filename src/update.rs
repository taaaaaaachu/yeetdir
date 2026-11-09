//! `yeetdir self-update`: GitHub Releases の最新版で自分自身の exe を置き換える。

use self_update::backends::github::Update;
use self_update::cargo_crate_version;

/// リリースの zip を zipsign で署名した鍵の公開鍵。秘密鍵は GitHub Secrets の ZIPSIGN_PRIVATE_KEY。
const VERIFYING_KEY: self_update::VerifyingKey = *include_bytes!("../signing-public.key");

fn updater() -> Result<Update, self_update::errors::Error> {
    Update::configure()
        .repo_owner("taaaaaaachu")
        .repo_name("yeetdir")
        .bin_name("yeetdir")
        // zip の中は yeetdir-v<版>-<target>/yeetdir.exe
        .bin_path_in_archive("yeetdir-v{{ version }}-{{ target }}/yeetdir.exe")
        .current_version(cargo_crate_version!())
        .verifying_keys([VERIFYING_KEY])
        .show_output(false)
        .no_confirm(true)
        .build()
}

/// 最新版に置き換え、結果を表示する。
pub fn run() -> Result<(), self_update::errors::Error> {
    let current = cargo_crate_version!();
    let status = updater()?.update()?;
    if status.is_updated() {
        eprintln!("{current} → {} に更新しました。", status.version());
    } else {
        eprintln!("最新版です ({current})。");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn updater_builds_with_embedded_key() {
        super::updater().expect("更新の設定が正しく組めること");
    }
}
