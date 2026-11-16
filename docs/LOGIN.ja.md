# dst へのログイン

dst は 2 種類の認証情報を `~/.dst/config.yml`（mode 600）に保存します。

| 認証情報 | 取得方法 | API へのヘッダー |
| --- | --- | --- |
| **DSH トークン**（個人利用で推奨） | `/login` の DeepSeek Platform WeChat QR ログイン | `x-dsh-auth-token: <token>` |
| **API キー** | 既存キー、または独自 endpoint | `Authorization: Bearer <key>` |

`/config` の行は `API key / token`、トークン保存時は値が `DSH token` になります。
API キーを設定（`/key`、config の入力、オンボーディング）するとトークンは
置き換えられ、新しいキーが隠れません。

![端末の WeChat ログイン画面](login-qr-screen.png)

## 3 つのログイン方法

`/login` を実行（または初回セットアップで *DeepSeek official* を選択）:

1. **端末に WeChat QR** — DeepSeek Platform 用の WeChat QR を取得しパネル右側に
   描画します。左側に番号付きガイドと `QR valid for m:ss` のカウントダウン。
   WeChat でスキャンしスマホで確認すると、端末がトークン交換を自動完了します。
2. **スマホのページ** — `http://<端末IP>:8787/`。同じ QR、進捗、カウントダウン、
   Cancel/Retry、API キー入力欄、既存 DSH トークンの貼り付け欄を表示。端末の
   UI 言語（en/zh/es/ja/de/it）に従います。
3. **直接入力** — DSH トークン、または Base URL + API キー + モデルを
   menuconfig 風に入力。

9 ステップは DSH デスクトップクライアントと同じ:
`wechat qrconnect → QR JPEG → スキャン長輪詢 → コールバック nonce →
Web セッショントークン → auth_init (PKCE) → auth_approve → auth_exchange →
DSH トークン`。

## 必要条件

- `open.weixin.qq.com`、`lp.open.weixin.qq.com`、`platform.deepseek.com`、
  `api.deepseek.com` へアクセスできること。
- システム時刻が正確（TLS 証明書検証）。
- パネルへの QR 描画には `python3` + Pillow **のみ** 必要。無い場合は
  スマホページ（同じ QR を表示）に自動フォールバック。
- ポート 8787 が空いていること（使用中なら一時ポート）。

## ログイン中の状態

| 状態 | 端末 | スマホページ |
| --- | --- | --- |
| 待機 | `→ QR valid for 4:50` | ローカライズされたカウントダウン |
| スキャン済み | `→ scanned — confirm on the phone` | ローカライズされた状態 |
| 確認中 | `→ authorized — finishing login…` | ローカライズされた状態 |
| 期限切れ | 1 回目は自動更新、2 回目は `login failed — press r to retry`（`r retry · t type · Esc cancel`） | *QR expired — refreshing…* と 1 回自動リトライ、Retry ボタンは残る |
| キャンセル | 端末で Esc | ページで Cancel |

## トラブルシューティング

| 症状 | 原因 / 対処 |
| --- | --- |
| QR が読めない | QR は WeChat 生成でピクセル単位に描画。`python3`+Pillow と画面の遮蔽を確認。ページにも同じ QR。 |
| `QR expired` が 2 回 | Wi-Fi/時計が不安定。時刻とネットを確認し `r`（再 QR）、`t`（トークン/API キー入力）、または Retry。 |
| `login failed` + HTTP/biz コード | ページにローカライズ要約と技術詳細を表示。後で再試行するか API キーを使用。 |
| `invalid dsh token` / `auth failed: use /key or /login` | トークン失効。`/login` を再実行、または `/key` で API キーに切替。 |
| スマホページが開かない | スマホと端末が同じ LAN、8787 が到達可能か確認（パネルの URL を使用）。 |
| 中国語/日本語が四角になる | 端末に CJK フォントが無い（`docs/UX-AUDIT.md`）。ページは動作する。 |

セキュリティ: DSH トークンはアカウントセッション相当です。端末内（mode 600）
のみに保存し、dst はアップロードしません。共用端末では公式 API キーを推奨。
この経路は DSH クライアントの非公開フローであり、変更される可能性があります。
