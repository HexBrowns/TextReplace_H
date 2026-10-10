//! 本体との読み書き。
//!
//! - 読み取りは `call_read_section`（本体の Undo に触れない）
//! - 書き込みは「置換」「改行」「そろえる」ボタンを押したときの `call_edit_section` 1 回だけ（本体の Undo 1 回分。
//!   中間点の表で、1 回の call_edit_section の書き込みが Ctrl+Z 1 回で戻ることを確かめた）。
//!   フォーカスの変化・入力中には書かない（ルール au2-rs-plugin）
//! - 第三者スクリプトの文字列項目（`--text@` はテキスト型、`--string@` は文字列型）も読める（第 2 段階）。
//!   どの項目が文字列かは本体の `get_effect_items` で調べ、効果名ごとに覚えておく

use std::collections::HashMap;
use std::sync::LazyLock;

use aviutl2::generic::{EffectItemType, ObjectHandle};
use parking_lot::RwLock;

use crate::EDIT_HANDLE;

/// 本体のテキストオブジェクトの効果名と、本文の項目名
pub const TEXT_EFFECT: &str = "テキスト";
pub const TEXT_ITEM: &str = "テキスト";

/// 書き換える項目（どの効果の、同名の何番目の、どの項目か）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    pub effect: String,
    /// 同名の効果の何番目か（0 始まり。`plugin2.h` の `"効果名:n"`）
    pub index: usize,
    pub item: String,
    pub kind: ItemKind,
}

/// 項目の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    /// 複数行（本体のテキストの本文と `--text@`）。改行は `\n`、`\` は `\\` で読み書きする（実機で確認）
    Text,
    /// 1 行（`--string@`）。`\` の書き方は未確認なので、`\` を含む値は扱わない
    String,
}

impl Target {
    pub fn builtin() -> Self {
        Target { effect: TEXT_EFFECT.into(), index: 0, item: TEXT_ITEM.into(), kind: ItemKind::Text }
    }

    pub fn is_builtin(&self) -> bool {
        self.effect == TEXT_EFFECT && self.item == TEXT_ITEM
    }

    /// 一覧に出す名前（本体のテキストの本文なら空）
    pub fn label(&self) -> String {
        if self.is_builtin() && self.index == 0 {
            return String::new();
        }
        let effect = if self.index > 0 { format!("{}（{} 個目）", self.effect, self.index + 1) } else { self.effect.clone() };
        format!("{effect} / {}", self.item)
    }
}

/// 効果名 → 文字列の項目（名前と種類）。取れなかった効果は空で覚える（毎回問い合わせない）
static STRING_ITEMS: LazyLock<RwLock<HashMap<String, Vec<(String, ItemKind)>>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// その効果の文字列の項目。`get_effect_items` は編集セクションの外で呼ぶ（ColorHistory_H と同じ）
pub fn string_items(effect: &str) -> Vec<(String, ItemKind)> {
    if let Some(v) = STRING_ITEMS.read().get(effect) {
        return v.clone();
    }
    let items: Vec<(String, ItemKind)> = if EDIT_HANDLE.is_ready() {
        EDIT_HANDLE
            .get_effect_items(effect)
            .map(|list| {
                list.into_iter()
                    .filter_map(|i| match i.item_type {
                        EffectItemType::Text => Some((i.name, ItemKind::Text)),
                        EffectItemType::String => Some((i.name, ItemKind::String)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    STRING_ITEMS.write().insert(effect.to_string(), items.clone());
    items
}

/// エイリアス（`[Object]` / `[Object.N]`）から、効果ごとの (効果名, 同名の何番目, 行) を取る
pub fn alias_effects(alias: &str) -> Vec<(String, usize, Vec<(String, String)>)> {
    let mut out: Vec<(String, usize, Vec<(String, String)>)> = Vec::new();
    let mut in_effect = false;
    for raw in alias.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_effect = name.strip_prefix("Object.").is_some_and(|n| n.parse::<usize>().is_ok());
            if in_effect {
                out.push((String::new(), 0, Vec::new()));
            }
            continue;
        }
        if !in_effect {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        if let Some(e) = out.last_mut() {
            if k == "effect.name" {
                e.0 = v.to_string();
            } else {
                e.2.push((k.to_string(), v.to_string()));
            }
        }
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for e in &mut out {
        let n = seen.entry(e.0.clone()).or_insert(0);
        e.1 = *n;
        *n += 1;
    }
    out
}

/// 効果の列から、第三者スクリプトの文字列の項目を拾う（本体のテキスト効果は除く。本文は別に読む）
pub fn script_strings(effects: &[(String, usize, Vec<(String, String)>)]) -> Vec<(Target, String)> {
    let mut out = Vec::new();
    for (name, index, items) in effects {
        if name.is_empty() || name == TEXT_EFFECT {
            continue;
        }
        let strings = string_items(name);
        if strings.is_empty() {
            continue;
        }
        for (key, value) in items {
            if let Some((_, kind)) = strings.iter().find(|(n, _)| n == key) {
                out.push((Target { effect: name.clone(), index: *index, item: key.clone(), kind: *kind }, value.clone()));
            }
        }
    }
    out
}

/// 表示中のシーンの文字列 1 つ
#[derive(Debug, Clone)]
pub struct LiveText {
    pub handle: ObjectHandle,
    pub layer: usize,
    pub start: usize,
    pub end: usize,
    pub target: Target,
    /// 保存形式のまま（`\n` は 2 文字）
    pub raw: String,
}

pub struct Scene {
    pub id: i32,
    pub name: String,
    pub texts: Vec<LiveText>,
}

fn ready() -> Result<(), String> {
    if EDIT_HANDLE.is_ready() {
        Ok(())
    } else {
        Err("本体の準備ができていません".into())
    }
}

/// 表示中のシーンのテキストを全部読む（効果名「テキスト」の項目「テキスト」）。
/// `scripts` なら、第三者スクリプトの文字列の項目も読む
pub fn read_current_scene(scripts: bool) -> Result<Scene, String> {
    ready()?;
    // 編集情報は読み取りセクションに入る前に取る（ColorHistory_H と同じ）
    let info = EDIT_HANDLE.get_edit_info();
    let (id, layer_max) = (info.scene_id, info.layer_max);
    type RawObject = (ObjectHandle, usize, usize, usize, Option<String>, Option<String>);
    let (name, objects) = EDIT_HANDLE
        .call_read_section(move |read| {
            let name = read.get_scene_name().unwrap_or_default();
            let mut objects: Vec<RawObject> = Vec::new();
            for layer in 0..=layer_max {
                for (lf, handle) in read.objects_in_layer(layer) {
                    let text = read.get_object_effect_item(handle, TEXT_EFFECT, 0, TEXT_ITEM).ok();
                    let alias = if scripts { read.object(handle).get_alias().ok() } else { None };
                    if text.is_some() || alias.is_some() {
                        objects.push((handle, lf.layer, lf.start, lf.end, text, alias));
                    }
                }
            }
            (name, objects)
        })
        .map_err(|e| format!("読み取りに失敗しました: {e}"))?;
    // 項目の種類の問い合わせは読み取りセクションの外で行う
    let mut texts = Vec::new();
    for (handle, layer, start, end, text, alias) in objects {
        if let Some(raw) = text {
            texts.push(LiveText { handle, layer, start, end, target: Target::builtin(), raw });
        }
        if let Some(alias) = alias {
            for (target, raw) in script_strings(&alias_effects(&alias)) {
                texts.push(LiveText { handle, layer, start, end, target, raw });
            }
        }
    }
    Ok(Scene { id, name, texts })
}

/// 1 つ分の書き込み
#[derive(Debug, Clone)]
pub struct Write {
    pub handle: ObjectHandle,
    pub target: Target,
    pub place: String,
    /// 検索したときの保存形式。書く直前に読み直して違えば書かない
    pub expected_raw: String,
    pub new_raw: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub written: usize,
    /// 検索した後に本体側で変わっていたので書かなかった
    pub stale: Vec<String>,
    /// 「そろえる」で、中間点の数が違うので写さなかった相手（場所と項目）
    pub points_mismatch: Vec<String>,
    pub errors: Vec<String>,
}

pub fn apply(writes: Vec<Write>) -> Result<Report, String> {
    ready()?;
    EDIT_HANDLE
        .call_edit_section(move |edit| {
            let mut report = Report::default();
            for w in writes {
                let t = &w.target;
                if !edit.object_exists(w.handle) {
                    report.errors.push(format!("{}: オブジェクトがありません", w.place));
                    continue;
                }
                match edit.get_object_effect_item(w.handle, &t.effect, t.index, &t.item) {
                    Ok(cur) if cur == w.expected_raw => {}
                    Ok(_) => {
                        report.stale.push(w.place);
                        continue;
                    }
                    Err(e) => {
                        report.errors.push(format!("{}: 読めません（{e}）", w.place));
                        continue;
                    }
                }
                if let Err(e) = edit.set_object_effect_item(w.handle, &t.effect, t.index, &t.item, &w.new_raw) {
                    report.errors.push(format!("{}: 書けません（{e}）", w.place));
                    continue;
                }
                match edit.get_object_effect_item(w.handle, &t.effect, t.index, &t.item) {
                    Ok(back) if back == w.new_raw => report.written += 1,
                    Ok(back) => {
                        tracing::warn!("[TextReplace_H] {} / {}: wrote {:?} read back {:?}", w.place, t.label(), w.new_raw, back);
                        report.errors.push(format!("{}: 書いた値と読み返しが違います", w.place));
                    }
                    // 読み返せなければ、入ったかを確かめられていない（ルール au2-rs-plugin「書き込みは読み返して確かめる」）
                    Err(e) => {
                        tracing::warn!("[TextReplace_H] {} / {}: read back failed: {e}", w.place, t.label());
                        report.errors.push(format!("{}: 書いた後に読み返せません（{e}）", w.place));
                    }
                }
            }
            report
        })
        .map_err(|e| format!("書き込みを始められませんでした: {e}"))
}

/// 一覧の行から、フォーカスを移す先（検索したときに読んだもの）
#[derive(Debug, Clone)]
pub struct FocusTarget {
    pub handle: ObjectHandle,
    /// 検索したときのシーン
    pub scene_id: i32,
    pub target: Target,
    /// 検索したときの保存形式
    pub raw: String,
    /// 検索したときのレイヤーと開始フレーム
    pub layer: usize,
    pub start: usize,
}

/// 検索したときと同じオブジェクトとみなせるか。項目が読めること（同じ種類のオブジェクト）に加えて、
/// 本文か位置（レイヤーと開始フレーム）のどちらかが検索したときと同じこと。
/// 検索の後に本文だけ直した・位置だけ動かしたものには移れるようにする
pub fn same_object(now_raw: Option<&str>, now_place: Option<(usize, usize)>, raw: &str, place: (usize, usize)) -> bool {
    match now_raw {
        None => false,
        Some(now) => now == raw || now_place == Some(place),
    }
}

/// 一覧の行をクリックしたときだけ、そのオブジェクトにフォーカスを移す。
/// ハンドルは検索の後に持ち越しているので、シーンと中身で同じオブジェクトかを確かめてから移す
/// （ルール au2-plugin-checklist「オブジェクトのハンドル」。ハンドルの値が使い回されるかは実機未確認）
pub fn focus(t: FocusTarget) -> Result<(), String> {
    ready()?;
    EDIT_HANDLE
        .call_edit_section(move |edit| -> Result<(), String> {
            if edit.info.scene_id != t.scene_id {
                return Err("検索したときとシーンが違います。検索したシーンに戻すか、検索し直してください".into());
            }
            if !edit.object_exists(t.handle) {
                return Err("オブジェクトが見つかりません。検索し直してください".into());
            }
            let now_raw = edit.get_object_effect_item(t.handle, &t.target.effect, t.target.index, &t.target.item).ok();
            let now_place = edit.get_object_layer_frame(t.handle).ok().map(|lf| (lf.layer, lf.start));
            if !same_object(now_raw.as_deref(), now_place, &t.raw, (t.layer, t.start)) {
                return Err("検索した後に本文も位置も変わっています。検索し直してください".into());
            }
            edit.set_focus_object(Some(t.handle)).map_err(|e| format!("フォーカスを移せませんでした: {e}"))
        })
        .map_err(|e| format!("フォーカスを移せませんでした: {e}"))?
}

/// トラックバーの値の、移動方法の前に並ぶ数値の数と、移動方法と、設定の先頭（ビット）。
/// 移動の無い値（数値 1 つ）やトラックバーでない値は None（BackupDiff_H の `live.rs` と同じ読み方）。
/// 設定の中にカンマが入るので、末尾からは数えない（ルール au2-rs-plugin「設定項目の値の形式」）
fn track_shape(value: &str) -> Option<(usize, &str, u32)> {
    let fields: Vec<&str> = value.split(',').map(str::trim).collect();
    let n = fields.iter().take_while(|f| f.parse::<f64>().is_ok()).count();
    if n == 0 || n == fields.len() {
        return None;
    }
    let bits = fields.get(n + 1).and_then(|s| s.split('|').next()).and_then(|b| b.parse::<u32>().ok()).unwrap_or(0);
    Some((n, fields[n], bits))
}

/// 値の数値の数が、写し先の点（開始・中間点・終了）の数と合うか。移動の無い値はどこにでも写せる。
/// 中間点無視（設定のビット 4）と再生範囲は値 2 つで保存されるので、2 つなら合うとみなす。
/// 本体は数を検査せずに保存するので、合わない値を書くと動きが黙って変わる
pub fn values_fit_points(value: &str, points: usize) -> bool {
    match track_shape(value) {
        None => true,
        Some((n, motion, bits)) => n == points || (n == 2 && (bits & 4 != 0 || motion == "再生範囲")),
    }
}

// ---- 見た目をそろえる（第 2 段階） ----

/// フォーカス中のオブジェクトのテキスト効果の項目（本文を除く）。テキストでなければ Err
pub fn read_style_source() -> Result<(ObjectHandle, Vec<(String, String)>), String> {
    ready()?;
    let names: Vec<String> = EDIT_HANDLE
        .get_effect_items(TEXT_EFFECT)
        .map_err(|e| format!("テキストの項目を調べられませんでした: {e:?}"))?
        .into_iter()
        .filter(|i| i.name != TEXT_ITEM && !matches!(i.item_type, EffectItemType::Group | EffectItemType::Separator | EffectItemType::NumberGroup))
        .map(|i| i.name)
        .collect();
    EDIT_HANDLE
        .call_read_section(move |read| -> Result<(ObjectHandle, Vec<(String, String)>), String> {
            let handle = read
                .get_focused_object()
                .map_err(|e| format!("フォーカス中のオブジェクトを読めませんでした: {e:?}"))?
                .ok_or("オブジェクトを 1 つ選んで（フォーカスして）ください")?;
            if read.get_object_effect_item(handle, TEXT_EFFECT, 0, TEXT_ITEM).is_err() {
                return Err("フォーカス中のオブジェクトはテキストではありません".into());
            }
            let items = names
                .into_iter()
                .filter_map(|n| read.get_object_effect_item(handle, TEXT_EFFECT, 0, &n).ok().map(|v| (n, v)))
                .collect();
            Ok((handle, items))
        })
        .map_err(|e| format!("読み取りに失敗しました: {e}"))?
}

/// 選択中のオブジェクトのうち、テキストのもの
pub fn read_selected_texts() -> Result<Vec<ObjectHandle>, String> {
    ready()?;
    EDIT_HANDLE
        .call_read_section(|read| -> Result<Vec<ObjectHandle>, String> {
            let handles = read.get_selected_objects().map_err(|e| format!("選択中のオブジェクトを読めませんでした: {e:?}"))?;
            Ok(handles.into_iter().filter(|h| read.get_object_effect_item(*h, TEXT_EFFECT, 0, TEXT_ITEM).is_ok()).collect())
        })
        .map_err(|e| format!("読み取りに失敗しました: {e}"))?
}

/// 「そろえる」ボタン専用。`targets` のテキスト効果の項目に `items` を書く（1 回の編集セクション = 本体の Undo 1 回分）。
/// 書く直前に選択を読み直し、読んだときと違えば何も書かない。
/// 移動しているトラックバーの値は、写し先の点の数と合わなければ、その写し先には何も書かずに `points_mismatch` に入れる
pub fn apply_style(targets: Vec<ObjectHandle>, items: Vec<(String, String)>) -> Result<Report, String> {
    ready()?;
    EDIT_HANDLE
        .call_edit_section(move |edit| -> Result<Report, String> {
            let now = edit.get_selected_objects().map_err(|e| format!("選択中のオブジェクトを読めませんでした: {e:?}"))?;
            let now: Vec<ObjectHandle> = now.into_iter().filter(|h| edit.get_object_effect_item(*h, TEXT_EFFECT, 0, TEXT_ITEM).is_ok()).collect();
            if now.len() != targets.len() || !targets.iter().all(|t| now.contains(t)) {
                return Err("選択が変わりました。「選択を読む」を押し直してください".into());
            }
            let mut report = Report::default();
            for (n, h) in targets.iter().enumerate() {
                let place = match edit.get_object_layer_frame(*h) {
                    Ok(lf) => format!("L{} {}-{}", lf.layer + 1, lf.start, lf.end),
                    Err(_) => format!("{} 個目", n + 1),
                };
                // 移動している値は、点の数が合う相手にだけ写す。合わなければこの相手には何も書かない
                // （ルール au2-plugin-checklist「編集の前後で保つもの」。中間点の数は値の数で決まる）
                let points = match edit.get_object_section_num(*h) {
                    Ok(s) => s + 1,
                    Err(e) => {
                        report.errors.push(format!("{place}: 中間点の数を読めません（{e}）"));
                        continue;
                    }
                };
                let misfit: Vec<&str> = items.iter().filter(|(_, v)| !values_fit_points(v, points)).map(|(i, _)| i.as_str()).collect();
                if !misfit.is_empty() {
                    tracing::warn!("[TextReplace_H] style {place}: points {points}, skipped for {misfit:?}");
                    report.points_mismatch.push(format!("{place}（{}）", misfit.join("・")));
                    continue;
                }
                let mut ok = true;
                for (item, value) in &items {
                    if let Err(e) = edit.set_object_effect_item(*h, TEXT_EFFECT, 0, item, value) {
                        report.errors.push(format!("{place} の {item}: 書けません（{e}）"));
                        ok = false;
                        continue;
                    }
                    match edit.get_object_effect_item(*h, TEXT_EFFECT, 0, item) {
                        Ok(back) if &back == value => {}
                        Ok(back) => {
                            tracing::warn!("[TextReplace_H] style {item}: wrote {value:?} read back {back:?}");
                            report.errors.push(format!("{place} の {item}: 書いた値と読み返しが違います"));
                            ok = false;
                        }
                        Err(e) => {
                            tracing::warn!("[TextReplace_H] style {item}: read back failed: {e}");
                            report.errors.push(format!("{place} の {item}: 書いた後に読み返せません（{e}）"));
                            ok = false;
                        }
                    }
                }
                if ok {
                    report.written += 1;
                }
            }
            Ok(report)
        })
        .map_err(|e| format!("書き込みを始められませんでした: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_effects_count_same_named() {
        let alias = "[Object]\r\nframe=0,59\r\n[Object.0]\r\neffect.name=縁取り文字@X\r\n文字=あ\r\n[Object.1]\r\neffect.name=縁取り文字@X\r\n文字=い\r\n[Object.2]\r\neffect.name=標準描画\r\nX=0.00\r\n";
        let e = alias_effects(alias);
        assert_eq!(e.len(), 3);
        assert_eq!((e[0].0.as_str(), e[0].1, e[0].2[0].1.as_str()), ("縁取り文字@X", 0, "あ"));
        assert_eq!((e[1].0.as_str(), e[1].1), ("縁取り文字@X", 1));
        assert_eq!((e[2].0.as_str(), e[2].1), ("標準描画", 0));
    }

    #[test]
    fn values_and_points() {
        // 移動の無い値・トラックバーでない値は、点の数によらず写せる
        assert!(values_fit_points("34.00", 4));
        assert!(values_fit_points("ffffff", 3));
        assert!(values_fit_points("000000", 3));
        assert!(values_fit_points("MS UI Gothic", 3));
        // 移動している値は点の数が同じときだけ
        assert!(values_fit_points("10.00,20.00,30.00,直線移動,0", 3));
        assert!(!values_fit_points("10.00,20.00,30.00,直線移動,0", 2));
        assert!(!values_fit_points("10.00,20.00,直線移動,0", 3));
        // 中間点無視と再生範囲は値 2 つ
        assert!(values_fit_points("10.00,20.00,直線移動,4", 5));
        assert!(values_fit_points("10.00,20.00,直線移動,6|", 5));
        assert!(values_fit_points("10.00,20.00,再生範囲,0", 3));
        // 設定の中のカンマ
        assert!(values_fit_points("0.00,100.00,プローブ移動_H,0|9,0,1", 2));
        assert!(!values_fit_points("0.00,100.00,プローブ移動_H,0|9,0,1", 3));
    }

    #[test]
    fn focus_same_object() {
        let place = (2, 30);
        // 本文も位置も同じ・本文だけ直した・位置だけ動かした、は同じオブジェクトとみなす
        assert!(same_object(Some("あ"), Some((2, 30)), "あ", place));
        assert!(same_object(Some("い"), Some((2, 30)), "あ", place));
        assert!(same_object(Some("あ"), Some((5, 90)), "あ", place));
        // 両方違う・項目が読めない（同じ種類のオブジェクトでない）は移らない
        assert!(!same_object(Some("い"), Some((5, 90)), "あ", place));
        assert!(!same_object(Some("い"), None, "あ", place));
        assert!(!same_object(None, Some((2, 30)), "あ", place));
    }

    #[test]
    fn target_labels() {
        assert_eq!(Target::builtin().label(), "");
        let t = Target { effect: "縁取り文字@X".into(), index: 1, item: "文字".into(), kind: ItemKind::String };
        assert_eq!(t.label(), "縁取り文字@X（2 個目） / 文字");
    }
}
