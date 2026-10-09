# TextReplace_H（テキスト置換）

テキストオブジェクトの本文を検索し、表示中のシーンはまとめて置換する AviUtl2 の汎用プラグイン。指定した文字数で改行することもできる。
ほかのシーンは保存済みの `.aup2` から「どこにあるか」を出すだけ（書き換えない）。

- 仕様: `AI/specifications/20261005_TextReplace_H_spec.md`
- 本体 2.1.11 以降（aviutl2-rs 0.48）

## ビルドと配置

```bash
python AI/tools/au2_build.py TextReplace_H                     # 本番へ
python AI/tools/au2_build.py TextReplace_H --deploy-root=C:/ProgramData/aviutl2/AI/devenv/aviutl2/data   # devenv へ
```

## 作りの要点

| ファイル | 中身 |
|---|---|
| `src/text.rs` | 保存形式との変換（`\n` `\\`）、制御文字と地の文の切り分け、検索・置換、文字数での改行（cargo test の中心） |
| `src/dict.rs` | 置換の辞書（`Plugin/TextReplace_H/辞書.json` の読み書きと、上から順に流す置換） |
| `src/aup2.rs` | 保存済みの `.aup2` からテキストオブジェクトの本文を読む（ColorHistory_H の `parse_aup2` を写して広げた） |
| `src/edit_ops.rs` | 表示中のシーンの読み取り（`call_read_section`）、書き込み（`call_edit_section` 1 回）、フォーカス |
| `src/gui.rs` | ウィンドウ（egui） |

- 書き込みはボタンを押したときだけ。書く直前に読み直し、検索の後に変わっていた本文は飛ばす。書いた後に読み返して確かめる
- 検索した後に条件を変えたら、書き込みを止めて検索し直してもらう
- 本体は改行を `\n` の 2 文字、`\` を `\\` で返す（実機で確認）。書くときもこの形にする（本文の `\` → `\\`、改行 → `\n`）。この形で表示も読み返しも合い、書き直しても `\` は増えないことを実機で確かめた（`AI/docs/20261008_パラメータ編集_SDK実機確認.md` の「テキストの `\`」）。書いた後の読み返しは、ほかの食い違いに備えて残す
