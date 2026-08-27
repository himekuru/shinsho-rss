# shinsho-rss

対象8レーベルの新刊を出版社公式サイトから取得し、1本のRSS 2.0フィードにまとめるRustプログラムです。

対象レーベル：

- 岩波新書
- 岩波ジュニア新書
- 中公新書
- 講談社現代新書
- ちくま新書
- ブルーバックス
- NHK出版新書
- ちくま学芸文庫

## GitHub Pagesで自動運用する

このプロジェクトには `.github/workflows/update-rss.yml` が入っています。GitHub Actionsが毎日06:15ごろ（日本時間）に実行され、RSSをGitHub Pagesへ公開します。GitHub側の混雑により開始時刻が遅れることはあります。

### 1. GitHubにリポジトリを作る

GitHubで新しいリポジトリを作り、このZIPの中身をリポジトリ直下へ置いてpushします。

最終的に次のような構成になっていればOKです。

```text
shinsho-rss/
├─ .github/
│  └─ workflows/
│     └─ update-rss.yml
├─ site/
│  └─ index.html
├─ src/
│  └─ main.rs
├─ .gitignore
├─ Cargo.toml
└─ README.md
```

### 2. GitHub Pagesを有効にする

リポジトリの `Settings` → `Pages` を開き、`Build and deployment` の `Source` を **GitHub Actions** にします。

### 3. 初回実行

リポジトリの `Actions` → `Update RSS` → `Run workflow` を押します。

初回実行では保存済み状態がないため、各レーベルの現在の新着ページに載っている本を登録します。2回目以降は、前回GitHub Pagesへ公開した `state.json` を読み戻して差分を管理します。

### 4. RSSリーダーに登録

PagesのURLが、たとえば

```text
https://USER.github.io/shinsho-rss/
```

ならRSS URLは

```text
https://USER.github.io/shinsho-rss/shinsho.xml
```

です。このURLをRSSリーダーに登録します。

## 状態の保存方法

`state.json` をGitリポジトリへ自動コミットする方式にはしていません。GitHub Pagesに `state.json` も一緒に公開し、次回のActions実行時にそれを取得します。

このため、リポジトリへの書き込み権限をActionsへ与える必要がなく、Actionsが自動コミットを繰り返すこともありません。

## 手元で実行する

Rustが入っている環境では次で実行できます。

```bash
cargo run --release
```

カレントディレクトリに次の2ファイルを作ります。

- `state.json` — 取得済み書誌
- `shinsho.xml` — RSS 2.0フィード

RSSのchannel linkを指定する場合は、環境変数 `FEED_LINK` を設定します。

```bash
FEED_LINK="https://example.com/shinsho.xml" cargo run --release
```

## 取得方式

- 岩波書店：岩波Web目録の `/api/search` を使用
- 中央公論新社：中公新書一覧を取得し、新規本だけ詳細ページからISBNを補完
- 講談社：講談社現代新書／ブルーバックスの一覧を取得し、新規本だけ詳細ページから紙版発売日・ISBNを補完
- 筑摩書房：ちくま新書／ちくま学芸文庫の一覧から書誌情報を取得
- NHK出版：新書一覧からNHK出版新書だけを抽出

1つの取得元が一時的に失敗しても、ほかの取得元の処理は続行します。8取得元すべてに失敗した場合だけ実行全体を失敗扱いにします。

## 注意

出版社サイトのHTMLやAPI仕様が変わった場合はselectorや取得処理の修正が必要です。取得頻度は日1回に設定しています。
