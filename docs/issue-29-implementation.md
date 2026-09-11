# Issue #29 実装・TUI整理

ブランチ: `feat/compact-resource-summary`。実装・ビルド済み。2026-09-12時点で見た目の調整を一区切りとし、Issue #29に完了範囲と検証結果を記録する。PR・マージ・リリースは別途扱う。

## 起動

WSLでリポジトリ直下から:

```bash
./target/release/wsltop --interactive
```

色を明示的に有効にする場合:

```bash
./target/release/wsltop --interactive --color always
```

従来の1行ヘッダー・単色表示:

```bash
./target/release/wsltop --interactive --header classic --color never
```

再ビルドは `cargo build --release --locked`。

## 実装内容

- 標準ヘッダーをCPU・RAMの2行に変更。全体値とWindows/WSL/WSLC/Dockerの観測値を表示する。
- CPU・RAMの全体値の横に履歴グラフを追加。左が過去、右が現在。0〜100%の固定尺度で、RAMは物理メモリの使用率を描く。
- 履歴は最大23点、1点は設定した更新間隔。CPU・RAM共通の固定時計に合わせて、全体が一斉に左へ移動する。計測待ちは前の値を保持する（新しい実測値ではない）。他の収集器の更新や再描画で履歴の時計を進めない。
- 履歴の初回計測前は空白、取得失敗・取得不能は成功するまで `!`、低負荷/ゼロは `▁`。`TERM=dumb` ではASCII表示にするが、失敗記号は同じ `!`。
- Windows収集の待機時間は「設定間隔−収集処理時間」。処理時間を含めた周期にする。処理が周期を超えたら終了後に次を開始し、収集を並列で重ねない。
- 両グラフを同じ固定幅に揃え、120列以上は23点、80〜119列は15点。80列未満はCPU/RAM全体値だけにする。非表示中も履歴は保持する。
- 全体値はラベル込み14桁、環境別の数値は単位込み6桁の欄とし、上下の列とグラフの位置を固定。欠測や桁数の変化でも後続列が動かない。
- 環境ラベルの色をヘッダーと一覧で統一。色を消しても名前と数値を維持する。
- ホストの物理RAMをWindowsのGlobalMemoryStatusExで取得。CPU計測の初回待機中でもRAMは表示できる。
- 表示件数・フィルター・ソート・一覧のCPUスケールを変えてもサマリーは変わらない。
- 未取得・無効化・収集失敗・不完全な収集はN/A。正常な空の収集結果はゼロにする。
- `obs` ラベルを削除し、その3文字分を履歴に回す。ラベルは `CPU` / `RAM` / `WSL` に短縮し、観測値の意味・primary・包含関係はhelpで説明。`?` で指標と操作の説明を開き、矢印/Pgでスクロールできる。
- Summary 2行、枠なしResource table、footer 1行の3層に整理。Resourcesタイトル行を削除し、view/sort/interval/optionをfooterへ移動。`cpu↓` / `mem↓` / `name↑` の短縮表記を使用し、幅不足では操作ヒントを段階的に省略する。flat/tree・sort・q quitは可能な限り残す。高さ不足では全体値だけの1行にする。classic指定では従来のview・Host CPU・CPU scale・sort・intervalの1行ヘッダーを表示する。
- 正常更新時の `updated` を削除。収集状態がある場合はfooterに `!`、幅に余裕があれば詳細も表示する。詳細はhelpでも確認できる。
- 最終polishで全体値を固定幅内の左揃えとし、CPU/RAMのvalue/bar開始位置を統一。環境ブロックの幅を固定し、120列以上では間隔3文字、80〜119列では1文字にする。Footerは `[flat cpu↓ core 3.0s]` とCPU scaleを左側に統合。狭幅では補助キー、interval、scaleの順に省略し、view/sort/qを維持する。
- Summary下にdimの区切り線を追加し、表の見出し下の線も維持する（上部は線込み5行）。高さが足りない場合は追加の線を省略。ASCII環境では `-`、単色指定では装飾なし。Footer左端は `[flat cpu↓ core 3.0s]` とまとめ、幅不足では補助キーを右側から省略する。
- `--header classic|compact`、`--color auto|always|never` を追加。autoはNO_COLOR/TERM=dumbを尊重し、alwaysは明示指定を優先する。
- 通常のテキスト・JSON出力形式は維持する。

## 集計の意味と初期範囲

これは合計可能な4分割ではなく、環境別の観測値。WindowsではVMホスト行を、Docker/WSLCではコンテナ内プロセス行を重ねて足さない。WSLはprimaryと収集対象の追加ディストリビューションを含み、WSL内のDockerプロセスはWSLにも含まれる。この意味はhelpで説明する。

RAMはWindows working set、WSL RSS、コンテナCLI統計で定義が異なり、共有ページもある。ホスト物理RAMとは別の指標として扱う。環境間の差分からWindowsや未帰属値を推測せず、積み上げメーターは追加していない。

`--wsl-only` のCPU観測値はWSL可視CPUを分母とすることをhelpで説明する。このモードのホストCPU/RAMは取得しない。

PR全体ではTUI・header・historyに加え、CLIオプションをmainに追加し、model・windows・monitor・streamにホストRAMの取得と受け渡し、summaryに環境別観測値の集計を追加している。streamでは履歴の状態管理とWindows収集周期に処理時間を含める変更も行った。renderは共有メモリ書式関数の公開範囲とテスト用snapshot初期化を変更した。CPU accountingの計算式、公開JSON schema/output、query/sort semantics、コンテナの親子グループ化は維持する。「変更ファイルがTUI周辺だけ」という制限とハッシュ比較は後段の見た目調整に限った確認であり、PR全体の範囲ではない。

## 検証結果

- PRレビュー対応: classicの従来ヘッダーを復元し、treeのresource rowsを区切り線幅の算出から除外。長いtree行とclassic表示の回帰テストを追加。
- Windows RAMの新しいP/Invoke経路は、Windows 11 Pro + WSL2実機で埋め込みスクリプトを実行して検証済み。[実測記録](validation/2026-09-12-summary-memory.md)を参照。
- Windows CIの正常終了コマンドテストが5秒の起動待ちで失敗したため、そのテストのみ30秒に緩和。製品のタイムアウトと専用タイムアウトテストは維持。

- Summaryの縦区切り `|` も横separatorと共通のDimスタイルに統一。数値・履歴・環境色への非適用と、単色指定時の装飾なしを幅別テストで確認。

- Summary下とtable header下の区切り線に共通のDimスタイルを適用。固定の暗色は追加せず、見出し文字とresource rowsのスタイルを維持。単色指定時は従来どおり装飾を付けない。描画テストで両線のDim一致と見出し・本文への非適用を確認。

- Summary下separatorをSummaryと表見出し・罫線の実効幅に合わせ、端末幅で上限を設定。長いCOMMANDやスクロールで伸縮させない。通常のwide表示では表の罫線と同じ97列で止まる。0/60/80/120/240列と初期表示をテストで確認。

- 最終alignment修正後、120/80/60列の実画面で上下の列位置・余白・scaleを統合したfooterを確認。プレビュー: `/tmp/wsltop-final-alignment-preview.svg`。q終了前後の `stty -g` が一致することも確認した。

- 区切り線追加後の実機120/80/60列で、Summary下の線・表の見出し下の線・角括弧付きfooterを確認。表示例: `/tmp/wsltop-summary-separator-preview.svg`。下記20/12点のキャプチャは前段階の記録。

- TUI整理後の実機120/80/60列で、履歴20/12点と狭幅での非表示、footerの短縮を確認。tree・memory・昇順への切り替え、help開閉、q終了も確認。実画面キャプチャ: `/tmp/wsltop-polish-preview.svg`。

- `cargo test --locked --all-targets`: 158件成功。
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: 成功。
- `cargo check --locked --target x86_64-pc-windows-gnu`: 成功。
- `cargo build --release --locked`: 成功。
- `git diff --check`: 成功。
- WSL上の実機TUIでWindowsホストCPU/物理RAMとWindows/WSL観測値の取得を確認。Docker/WSLCは空の収集結果としてゼロを表示した。
- 40/79/80/119/120/160列の描画テストで3層構成、summaryの幅別表示、ENV色、footer必須項目を確認。4行以上では2行summaryと1行footerを確保する。
- 履歴の並び順、0/100%、取得失敗と計測待ちの区別、前値保持、共通時計での左移動、保持数の上限、他収集器の更新・フィルター操作で履歴を追加しないことをテストで検証。
- CPU/RAMの履歴幅と `obs`・Win・WSL・WSLC・Dockerの開始位置が上下で一致し、N/A・桁数・メモリ単位の変化でも列位置が動かないことをテストで検証。flat/treeの表内容を既存rendererと比較し、列とコンテナの親子表示を維持することも確認。
- 周期・前値保持の修正後、実機を約20秒観測。CPUの数値更新が約3秒間隔となり、正常収集中の履歴に欠測点が入らないことと、上下の列位置を維持することを確認。
- 従来表示、WSL-only表示、ヘルプの開閉、tree切り替えを確認。
- NO_COLORが設定された実機で、auto/neverの単色出力とalwaysの色出力を端末のANSI属性でも確認。

Windows用の実行ファイル生成は、このWSL環境に `x86_64-w64-mingw32-dlltool` がないため完了していない。Windows向け型チェックは成功しているが、Windowsネイティブ実行・MSVCビルドは別途確認が必要。稼働中コンテナの負荷を伴う実機検証と、明暗テーマそれぞれの見た目の確認も残る。

## 主なファイル

- `src/header.rs`: ヘッダー描画、幅の調整、色、指標説明。
- `src/history.rs`: 共通時計の固定スロット、前値保持、取得失敗・回復の扱い。
- `src/summary.rs`: 表示前データからの環境別観測値集計。
- `src/stream.rs`: 収集状態の反映、ホストRAMの受け渡し。
- `src/windows.rs` / `src/model.rs`: ホストRAMの取得と検証。
- `src/tui.rs` / `src/main.rs`: 画面・ヘルプ・オプションの接続。
- `README.md` / `CHANGELOG.md`: 使用方法と指標の定義。
