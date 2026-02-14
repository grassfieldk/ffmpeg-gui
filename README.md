# FFmpeg GUI

Tauri + TypeScript で構築する Windows 向け FFmpeg GUI の実装開始版です。

## 実装済み

- 1画面UIの骨組み（入力・変換設定・出力・ログ）
- ドラッグ＆ドロップ（1件）
- 複数ドロップ時の1件制約ガード
- `ffprobe` で入力動画の情報を取得し初期値に反映
- プリセット（編集用 / SNS投稿用）
- 項目ごとのリセット
- FFmpeg 初回ダウンロード（Gyan.dev）+ SHA-256 検証
- オフライン時の PATH フォールバック
- 変換コマンドのプレビュー
- 変換実行とログの逐次表示
- 変換進捗（%）表示
- 変換キャンセル
- エラー分類（ダウンロード失敗 / ハッシュ不一致 / 実行失敗）

## これから実装

- 失敗時ハンドリングの強化

## 変換中操作

- 実行中は「変換をキャンセル」で停止要求を送信できます

## セットアップ

```bash
npm install
npm run tauri dev
```

## FFmpeg取得ポリシー

- URL: `https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip`
- Version: `8.0.1`
- SHA-256: `e2aaeaa0fdbc397d4794828086424d4aaa2102cef1fb6874f6ffd29c0b88b673`
