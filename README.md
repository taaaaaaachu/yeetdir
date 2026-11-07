# yeetdir

[![CI](https://github.com/taaaaaaachu/yeetdir/actions/workflows/ci.yml/badge.svg)](https://github.com/taaaaaaachu/yeetdir/actions/workflows/ci.yml)

`yeetdir` は、指定したフォルダの中身を並列で削除する Windows 向け CLI です。
エクスプローラーの右クリックメニューに登録すると、フォルダを選んでそのまま削除できます。

## 注意

削除したファイルやフォルダはゴミ箱に入りません。OneDrive などの同期フォルダで消すと、クラウド側や他の端末からも消えます。

安全のため、次のようにしています。

- 削除の前に対象のフォルダを表示し、`y` と答えたときだけ削除します。確認を省略するには `--yes` を付けます。入力ができない環境(パイプやスクリプトから実行した場合など)では、`--yes` がないと中止します。
- ジャンクションやシンボリックリンクはたどらず、リンク自体だけを消します。リンクを削除対象として直接指定した場合は拒否します。
- ドライブのルート、ユーザーフォルダ、`C:\Windows`、`Program Files`、`ProgramData`、`AppData` と、それらの祖先(`C:\Users` など)は削除できません。
- ファイルの一覧を取ってから消すまでの間に、途中のフォルダをリンクへ差し替えられると、木の外を消せてしまいます。ほかのユーザーが書き込めるフォルダを、管理者権限で消すことは避けてください。

## 入手

[Releases](https://github.com/taaaaaaachu/yeetdir/releases) から `yeetdir-<版>-x86_64-pc-windows-msvc.zip` をダウンロードし、中の `yeetdir.exe` を好きな場所に置いてください。

## ビルド

Rust 1.85 以降が必要です。

```powershell
cargo build --release
```

生成された実行ファイルは `target\release\yeetdir.exe` です。

## CLI で使う

フォルダの中身だけを削除します。対象を表示して確認を求めます。

```powershell
yeetdir C:\path\to\dir
```

確認なしで削除する場合は `--yes` を付けます。

```powershell
yeetdir --yes C:\path\to\dir
```

複数フォルダも指定できます。指定した順番にフォルダごと処理します。

```powershell
yeetdir C:\path\to\dir1 C:\path\to\dir2
```

中身を削除したあと、指定したフォルダ自体も削除します。

```powershell
yeetdir --remove-root C:\path\to\dir
```

スレッド数を指定する場合は `--jobs` を使います。

```powershell
yeetdir --jobs 8 C:\path\to\dir
```

## エクスプローラーの右クリックに登録する

```powershell
yeetdir install-context-menu
```

登録後、フォルダを右クリックすると `yeetdir で削除…` が表示されます。
Windows 11 では `その他のオプションを確認` の中に表示されることがあります。

右クリックから実行した場合は、確認のあと、指定したフォルダ自体も削除します。失敗したときは、Enter キーを押すまでウィンドウを閉じません。中身だけを削除したい場合は CLI で `--contents-only` を指定してください。

```powershell
yeetdir --contents-only C:\path\to\dir
```

## 複数フォルダを右クリックした場合

複数のフォルダを選んで右クリックすると、Windows は対象ごとに `yeetdir.exe` を起動します。各プロセスは `%LOCALAPPDATA%\yeetdir\context-menu-queue` に対象のパスを置き、ロックを取れた 1 つのプロセスがまとめて確認・削除します。ほかのプロセスはすぐに終了しますが、選んだ数だけコンソールウィンドウが一瞬開きます。

60 秒より古いキューの項目は、過去の操作の取り残しとみなし、削除せずに捨てます。

## 登録を解除する

```powershell
yeetdir uninstall-context-menu
```

右クリックメニューの登録と、`%LOCALAPPDATA%\yeetdir` を削除します。

## オプション

```text
PATH...          削除対象のディレクトリ
-r, --remove-root
                 中身を削除したあと、指定したディレクトリ自体も削除する
--contents-only  エクスプローラー経由でもルートフォルダを残す
-y, --yes        削除前の確認を省略する
--pause          失敗したとき、Enter キーが押されるまでウィンドウを閉じない
-j, --jobs <N>   使うスレッド数
-p, --progress   標準エラーがターミナル以外でも進捗を出す
-q, --no-progress
                 進捗を出さない
```


## ライセンス

MIT
