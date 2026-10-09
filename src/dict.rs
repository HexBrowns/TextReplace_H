//! 置換の辞書（第 2 段階）
//!
//! よく直す表記ゆれ（「ｳﾏ娘」→「ウマ娘」など）の組を保存しておき、上から順にまとめて流す。
//! 保存先は `Plugin/TextReplace_H/辞書.json`（本体のデータフォルダから求める）。本体には何も書かない

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::text::{self, Options};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub find: String,
    pub replacement: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

pub fn default_path() -> PathBuf {
    aviutl2::config::app_data_path().join("Plugin").join("TextReplace_H").join("辞書.json")
}

/// 読む。ファイルが無ければ空。壊れていれば Err（上書きして消さないため）
pub fn load(path: &Path) -> Result<Vec<Entry>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(s.trim_start_matches('\u{feff}')).map_err(|e| format!("辞書を読めませんでした（{}）: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("辞書を読めませんでした（{}）: {e}", path.display())),
    }
}

/// 書く。一時ファイルに書いてから置き換える（途中で失敗しても元の辞書を壊さない）
pub fn save(path: &Path, entries: &[Entry]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("辞書のフォルダを作れませんでした: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("辞書を書けませんでした: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("辞書を書けませんでした: {e}"))
}

/// 有効な組を上から順に流す。戻り値は（新しい本文、置き換えた数）。前の組で置き換えた結果に後ろの組も当たる
pub fn apply(text: &str, entries: &[Entry], opts: &Options) -> Result<(String, usize), String> {
    let mut cur = text.to_string();
    let mut total = 0;
    for e in entries.iter().filter(|e| e.enabled && !e.find.is_empty()) {
        let (next, n) = text::replace(&cur, &e.find, &e.replacement, opts)
            .map_err(|err| format!("辞書の「{}」: {err}", e.find))?;
        cur = next;
        total += n;
    }
    Ok((cur, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(find: &str, rep: &str) -> Entry {
        Entry { find: find.into(), replacement: rep.into(), enabled: true }
    }

    #[test]
    fn applies_in_order_and_skips_disabled() {
        let o = Options { fold_width: true, ..Options::default() };
        let mut entries = vec![e("ｳﾏ娘", "ウマ娘"), e("トレーナーさん", "トレーナー"), e("", "x")];
        let (t, n) = apply("ウマ娘のﾄﾚｰﾅｰさんと<#ff0000>ｳﾏ娘<#>", &entries, &o).unwrap();
        assert_eq!((t.as_str(), n), ("ウマ娘のトレーナーと<#ff0000>ウマ娘<#>", 3));
        entries[1].enabled = false;
        assert_eq!(apply("トレーナーさん", &entries, &o).unwrap(), ("トレーナーさん".to_string(), 0));
    }

    #[test]
    fn bad_regex_names_the_entry() {
        let o = Options { regex: true, ..Options::default() };
        let err = apply("x", &[e("(", "")], &o).unwrap_err();
        assert!(err.contains("「(」"), "{err}");
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("text_replace_h_dict_{}", std::process::id()));
        let path = dir.join("辞書.json");
        assert_eq!(load(&path).unwrap(), Vec::<Entry>::new());
        let entries = vec![e("a", "b"), Entry { enabled: false, ..e("c", "d") }];
        save(&path, &entries).unwrap();
        assert_eq!(load(&path).unwrap(), entries);
        std::fs::write(&path, "{壊れた").unwrap();
        assert!(load(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
