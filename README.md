# yeetdir

`yeetdir` は、指定したフォルダの中身を並列で削除する Windows 向け CLI です。
エクスプローラーの右クリックメニューに登録すると、フォルダを選んでそのまま削除できます。

## 注意

削除したファイルやフォルダはゴミ箱に入りません。対象を確認してから実行してください。

## ビルド

```powershell
cargo build --release
```

生成された実行ファイルは `target\release\yeetdir.exe` です。

## CLI で使う

フォルダの中身だけを削除します。

```powershell
yeetdir C:\path\to\dir
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

右クリックから実行した場合は、指定したフォルダ自体も削除します。中身だけを削除したい場合は CLI で `--contents-only` を指定してください。

```powershell
yeetdir --contents-only C:\path\to\dir
```

## 複数フォルダを右クリックした場合

Windows の通常のレジストリ登録では、複数フォルダを選択して右クリックすると対象ごとにプロセスが起動し、ターミナルも複数開くことがあります。

`yeetdir` は登録時に `%LOCALAPPDATA%\yeetdir\context-menu-run.ps1` へ仲介スクリプトを作成します。複数選択時は各起動がいったんキューへ対象パスを書き込み、最初にロックを取ったスクリプトだけが短時間待ってから、まとめて `yeetdir.exe --remove-root <paths...>` を 1 回だけ起動します。

そのため、複数フォルダを選んで右クリックしても、1 つのターミナルで順番に削除されます。

## 登録を解除する

```powershell
yeetdir uninstall-context-menu
```

右クリックメニューの登録と、登録時に作成した仲介スクリプトを削除します。

## オプション

```text
PATH...          削除対象のディレクトリ
-r, --remove-root
                 中身を削除したあと、指定したディレクトリ自体も削除する
--contents-only  エクスプローラー経由でもルートフォルダを残す
-j, --jobs <N>   使うスレッド数
-p, --progress   標準エラーがターミナル以外でも進捗を出す
-q, --no-progress
                 進捗を出さない
```

