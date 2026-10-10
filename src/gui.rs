//! テキスト置換のウィンドウ（egui）
//!
//! 「検索」で一覧を作り、チェックした行を「置換」「改行」ボタンでまとめて書く（本体の Undo 1 回分）。
//! 検索した後に条件を変えたら、書き込みを止めて検索し直してもらう（一覧と書く中身がずれないように）。
//! 第 2 段階: 置換の辞書、第三者スクリプトの文字列の項目、見た目（フォント・色など）をそろえる

use std::path::PathBuf;
use std::sync::Arc;

use aviutl2::generic::ObjectHandle;
use aviutl2_eframe::{eframe, egui, AviUtl2EframeHandle};
use parking_lot::RwLock;

use crate::aup2;
use crate::dict::{self, Entry};
use crate::edit_ops::{self, FocusTarget, ItemKind, Target, Write};
use crate::text::{self, Options, WrapOptions};

const YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 200, 90);
const RED: egui::Color32 = egui::Color32::from_rgb(235, 110, 110);
const GREEN: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);
const PREVIEW: usize = 60;

/// 見た目をそろえるときに、既定で写す項目
const STYLE_DEFAULTS: &[&str] = &["フォント", "文字色", "影・縁色", "文字装飾"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Replace,
    Wrap,
    Dictionary,
    Style,
}

/// 検索したときの条件（変わったら書き込みを止める）
#[derive(Debug, Clone, PartialEq)]
struct Query {
    mode: Mode,
    find: String,
    replacement: String,
    opts: Options,
    wrap: WrapOptions,
    all_scenes: bool,
    /// 第三者スクリプトの文字列の項目も探す
    scripts: bool,
    /// 辞書で検索したときの辞書
    dict: Vec<Entry>,
}

struct Row {
    /// 表示中のシーン（書き換えられる）なら Some
    handle: Option<ObjectHandle>,
    target: Target,
    scene: String,
    layer: usize,
    start: usize,
    end: usize,
    raw: String,
    before: String,
    after: String,
    hits: usize,
    checked: bool,
}

impl Row {
    fn place(&self) -> String {
        let label = self.target.label();
        let label = if label.is_empty() { String::new() } else { format!(" {label}") };
        format!("{} L{} {}-{}{label}", self.scene, self.layer + 1, self.start, self.end)
    }
}

/// 見た目をそろえる画面の状態
#[derive(Default)]
struct StyleState {
    /// 元にしたオブジェクトと、その項目（写すか, 項目名, 値）
    source: Option<(ObjectHandle, Vec<(bool, String, String)>)>,
    /// 「選択を読む」で読んだ写し先
    targets: Option<Vec<ObjectHandle>>,
}

pub struct TextReplaceApp {
    _handle: AviUtl2EframeHandle,
    project: Arc<RwLock<Option<PathBuf>>>,
    q: Query,
    searched: Option<Query>,
    /// 検索したときに表示していたシーン（一覧の書き換えられる行はこのシーンのもの）
    searched_scene: Option<i32>,
    rows: Vec<Row>,
    notes: Vec<String>,
    status: String,
    status_is_error: bool,
    dict_path: PathBuf,
    /// 辞書（編集中のもの。変えたらすぐ保存する）
    dict: Vec<Entry>,
    /// 辞書を読めなかったとき（壊れていたら上書きしない）
    dict_error: Option<String>,
    style: StyleState,
}

impl TextReplaceApp {
    pub fn new(cc: &eframe::CreationContext<'_>, handle: AviUtl2EframeHandle, project: Arc<RwLock<Option<PathBuf>>>) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.visuals = aviutl2_eframe::aviutl2_visuals();
        });
        cc.egui_ctx.set_fonts(aviutl2_eframe::aviutl2_fonts());
        let dict_path = dict::default_path();
        let (dict, dict_error) = match dict::load(&dict_path) {
            Ok(d) => (d, None),
            Err(e) => {
                tracing::warn!("[TextReplace_H] {e}");
                (Vec::new(), Some(e))
            }
        };
        Self {
            _handle: handle,
            project,
            q: Query {
                mode: Mode::Replace,
                find: String::new(),
                replacement: String::new(),
                opts: Options::default(),
                wrap: WrapOptions::default(),
                all_scenes: true,
                scripts: false,
                dict: Vec::new(),
            },
            searched: None,
            searched_scene: None,
            rows: Vec::new(),
            notes: Vec::new(),
            status: String::new(),
            status_is_error: false,
            dict_path,
            dict,
            dict_error,
            style: StyleState::default(),
        }
    }

    fn set_status(&mut self, s: impl Into<String>, error: bool) {
        self.status = s.into();
        self.status_is_error = error;
    }

    fn save_dict(&mut self) {
        if self.dict_error.is_some() {
            return; // 壊れた辞書を空で上書きしない
        }
        if let Err(e) = dict::save(&self.dict_path, &self.dict) {
            self.set_status(e, true);
        }
    }

    /// 1 つの本文について、書き換えた後の本文と当たった数。対象外なら None
    fn transform(q: &Query, before: &str) -> Result<Option<(String, usize)>, String> {
        match q.mode {
            Mode::Replace => {
                let (after, n) = text::replace(before, &q.find, &q.replacement, &q.opts)?;
                Ok((n > 0).then_some((after, n)))
            }
            Mode::Dictionary => {
                let (after, n) = dict::apply(before, &q.dict, &q.opts)?;
                Ok((n > 0 && after != before).then_some((after, n)))
            }
            Mode::Wrap => {
                // 検索語があれば、それに当たる本文だけを対象にする
                let hits = if q.find.is_empty() { 1 } else { text::find(before, &q.find, &q.opts)?.len() };
                if hits == 0 {
                    return Ok(None);
                }
                let after = text::wrap(before, &q.wrap);
                Ok((after != before).then_some((after, hits)))
            }
            Mode::Style => Ok(None),
        }
    }

    /// 1 つの値を一覧の行にする。対象外なら None。文字列型の項目の扱えない値は `skipped` を数える
    #[allow(clippy::too_many_arguments)]
    fn make_row(
        q: &Query,
        handle: Option<ObjectHandle>,
        target: Target,
        scene: &str,
        (layer, start, end): (usize, usize, usize),
        raw: String,
        skipped: &mut usize,
    ) -> Result<Option<Row>, String> {
        // 文字列型（--string@）の `\` の書き方は未確認なので、`\` を含む値は読むだけにもしない
        if target.kind == ItemKind::String && raw.contains('\\') {
            *skipped += 1;
            return Ok(None);
        }
        let before = text::unescape(&raw);
        let Some((after, hits)) = Self::transform(q, &before)? else { return Ok(None) };
        if target.kind == ItemKind::String && (after.contains('\\') || after.contains('\n')) {
            *skipped += 1;
            return Ok(None);
        }
        let checked = handle.is_some();
        Ok(Some(Row { handle, target, scene: scene.to_string(), layer, start, end, raw, before, after, hits, checked }))
    }

    fn search(&mut self) {
        let mut q = self.q.clone();
        if q.mode == Mode::Dictionary {
            q.dict = self.dict.clone();
            if !q.dict.iter().any(|e| e.enabled && !e.find.is_empty()) {
                self.set_status("辞書に有効な組がありません", true);
                return;
            }
            self.q.dict = q.dict.clone();
        }
        if q.mode == Mode::Replace && q.find.is_empty() {
            self.set_status("検索する語を入れてください", true);
            return;
        }
        let scene = match edit_ops::read_current_scene(q.scripts) {
            Ok(s) => s,
            Err(e) => {
                self.set_status(e, true);
                return;
            }
        };
        let mut rows = Vec::new();
        let mut notes = Vec::new();
        let mut skipped = 0;
        let total_live = scene.texts.len();
        for t in scene.texts {
            match Self::make_row(&q, Some(t.handle), t.target, &scene.name, (t.layer, t.start, t.end), t.raw, &mut skipped) {
                Ok(Some(r)) => rows.push(r),
                Ok(None) => {}
                Err(e) => {
                    self.set_status(e, true);
                    return;
                }
            }
        }
        if q.all_scenes {
            let path = self.project.read().clone();
            match path {
                None => notes.push("プロジェクトが未保存なので、ほかのシーンは探していません".to_string()),
                Some(p) => match std::fs::read(&p) {
                    Err(e) => notes.push(format!("保存済みのプロジェクトを読めませんでした: {e}")),
                    Ok(bytes) => {
                        let saved = aup2::parse(&String::from_utf8_lossy(&bytes));
                        let before_len = rows.len();
                        let name_of = |id: i32| saved.scenes.get(&id).cloned().unwrap_or_else(|| format!("シーン {id}"));
                        for t in saved.texts.iter().filter(|t| t.scene != scene.id) {
                            let span = (t.layer, t.start, t.end);
                            if let Ok(Some(r)) = Self::make_row(&q, None, Target::builtin(), &name_of(t.scene), span, t.raw.clone(), &mut skipped) {
                                rows.push(r);
                            }
                        }
                        if q.scripts {
                            for o in saved.objects.iter().filter(|o| o.scene != scene.id) {
                                for (target, raw) in edit_ops::script_strings(&o.effects) {
                                    let span = (o.layer, o.start, o.end);
                                    if let Ok(Some(r)) = Self::make_row(&q, None, target, &name_of(o.scene), span, raw, &mut skipped) {
                                        rows.push(r);
                                    }
                                }
                            }
                        }
                        if rows.len() > before_len {
                            notes.push("ほかのシーンは最後に保存した内容です（見るだけ。書き換えられるのは表示中のシーンだけ）".to_string());
                        }
                    }
                },
            }
        }
        if skipped > 0 {
            notes.push(format!("文字列（1 行）の項目で \\ か改行を含むもの {skipped} 個は対象から外しました（書き方を確かめていないため）"));
        }
        rows.sort_by_key(|r| (r.handle.is_none(), r.scene.clone(), r.layer, r.start, r.target.label()));
        let live = rows.iter().filter(|r| r.handle.is_some()).count();
        let what = if q.scripts { "テキスト・文字列" } else { "テキスト" };
        self.set_status(format!("表示中のシーンの{what} {total_live} 個のうち {live} 個が対象（ほかのシーン {} 個）", rows.len() - live), false);
        tracing::info!("[TextReplace_H] 検索: 表示中 {total_live} 個中 {live} 個、ほかのシーン {} 個", rows.len() - live);
        self.rows = rows;
        self.notes = notes;
        self.searched = Some(q);
        self.searched_scene = Some(scene.id);
    }

    fn apply(&mut self) {
        let writes: Vec<Write> = self
            .rows
            .iter()
            .filter(|r| r.checked)
            .filter_map(|r| {
                r.handle.map(|h| Write {
                    handle: h,
                    target: r.target.clone(),
                    place: r.place(),
                    expected_raw: r.raw.clone(),
                    new_raw: text::escape(&r.after),
                })
            })
            .collect();
        if writes.is_empty() {
            return;
        }
        let n = writes.len();
        match edit_ops::apply(writes) {
            Ok(rep) => {
                tracing::info!("[TextReplace_H] 書き込み: {} / {n} 個（飛ばした {}・失敗 {}）", rep.written, rep.stale.len(), rep.errors.len());
                let mut msg = format!("{} / {n} 個を書き換えました", rep.written);
                if !rep.stale.is_empty() {
                    msg.push_str(&format!("。検索の後に本体で変わっていたので飛ばしたもの: {}", rep.stale.join("、")));
                }
                if !rep.errors.is_empty() {
                    msg.push_str(&format!("。失敗: {}", rep.errors.join("、")));
                }
                let error = !rep.stale.is_empty() || !rep.errors.is_empty();
                // 一覧を今の内容で作り直す（書いたものは対象から外れる）
                self.search();
                self.set_status(msg, error);
            }
            Err(e) => self.set_status(e, true),
        }
    }

    fn render_dictionary(&mut self, ui: &mut egui::Ui) {
        if let Some(e) = &self.dict_error {
            ui.colored_label(RED, format!("{e}。直すまで辞書は保存しません"));
        }
        let mut changed = false;
        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        let len = self.dict.len();
        egui::ScrollArea::vertical().id_salt("dict").max_height(160.0).show(ui, |ui| {
            egui::Grid::new("dict_grid").num_columns(5).striped(true).show(ui, |ui| {
                ui.weak("使う");
                ui.weak("検索");
                ui.weak("置換");
                ui.weak("");
                ui.weak("");
                ui.end_row();
                for (i, e) in self.dict.iter_mut().enumerate() {
                    changed |= ui.checkbox(&mut e.enabled, "").changed();
                    changed |= ui.add(egui::TextEdit::singleline(&mut e.find).desired_width(130.0)).changed();
                    changed |= ui.add(egui::TextEdit::singleline(&mut e.replacement).desired_width(130.0)).changed();
                    ui.horizontal(|ui| {
                        if ui.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text("上へ（上から順に流します）").clicked() {
                            swap = Some((i - 1, i));
                        }
                        if ui.add_enabled(i + 1 < len, egui::Button::new("↓").small()).on_hover_text("下へ").clicked() {
                            swap = Some((i, i + 1));
                        }
                    });
                    if ui.small_button("削除").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some((a, b)) = swap {
            self.dict.swap(a, b);
            changed = true;
        }
        if let Some(i) = remove {
            self.dict.remove(i);
            changed = true;
        }
        ui.horizontal(|ui| {
            if ui.button("組を足す").clicked() {
                self.dict.push(Entry { find: String::new(), replacement: String::new(), enabled: true });
                changed = true;
            }
            ui.weak(format!("{} 組。上から順に置き換えます。保存先 {}", self.dict.len(), self.dict_path.display()));
        });
        if changed {
            self.save_dict();
        }
    }

    fn render_top(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.q.mode, Mode::Replace, "置換");
            ui.selectable_value(&mut self.q.mode, Mode::Dictionary, "辞書");
            ui.selectable_value(&mut self.q.mode, Mode::Wrap, "文字数で改行");
            ui.selectable_value(&mut self.q.mode, Mode::Style, "見た目をそろえる");
        });
        if self.q.mode == Mode::Style {
            return;
        }
        match self.q.mode {
            Mode::Replace => {
                egui::Grid::new("query").num_columns(2).show(ui, |ui| {
                    ui.label("検索");
                    ui.add(egui::TextEdit::singleline(&mut self.q.find).desired_width(260.0));
                    ui.end_row();
                    ui.label("置換");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.q.replacement).desired_width(260.0));
                        if ui.add_enabled(!self.q.find.is_empty(), egui::Button::new("辞書に足す")).clicked() {
                            self.dict.push(Entry { find: self.q.find.clone(), replacement: self.q.replacement.clone(), enabled: true });
                            self.save_dict();
                            self.set_status(format!("辞書に「{} → {}」を足しました", self.q.find, self.q.replacement), false);
                        }
                    });
                    ui.end_row();
                });
            }
            Mode::Dictionary => self.render_dictionary(ui),
            Mode::Wrap => {
                egui::Grid::new("query").num_columns(2).show(ui, |ui| {
                    ui.label("絞り込み");
                    ui.add(egui::TextEdit::singleline(&mut self.q.find).desired_width(260.0).hint_text("空ならすべてのテキスト"));
                    ui.end_row();
                    ui.label("1 行の文字数");
                    ui.add(egui::DragValue::new(&mut self.q.wrap.width).range(1.0..=500.0).speed(0.5));
                    ui.end_row();
                });
            }
            Mode::Style => {}
        }
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.q.opts.case_sensitive, "大文字小文字を区別");
            ui.add_enabled(!self.q.opts.regex, egui::Checkbox::new(&mut self.q.opts.fold_width, "全角と半角を同じとみなす"));
            ui.checkbox(&mut self.q.opts.regex, "正規表現").on_hover_text("置換語では $1 などが使えます");
            ui.checkbox(&mut self.q.opts.skip_tags, "制御文字の中は探さない").on_hover_text("<#色> <s32> <$プリセット> などの中は検索・置換しない");
            ui.checkbox(&mut self.q.all_scenes, "ほかのシーンも探す（保存済み・見るだけ）");
            ui.checkbox(&mut self.q.scripts, "スクリプトの文字列も探す")
                .on_hover_text("テキスト系スクリプトの文字の項目（--text@ / --string@）も対象にします。一覧に効果名と項目名が出ます");
        });
        if self.q.mode == Mode::Wrap {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.q.wrap.half_as_half, "半角は 0.5 文字");
                ui.checkbox(&mut self.q.wrap.keep_newlines, "今の改行を残す").on_hover_text("外すと、今の改行を消してから折り返します");
                ui.checkbox(&mut self.q.wrap.kinsoku, "行頭禁則").on_hover_text("句読点や閉じ括弧で行を始めない（その行が 1 文字はみ出します）");
            });
        }
        ui.horizontal(|ui| {
            let label = if self.q.mode == Mode::Dictionary { "辞書で検索" } else { "検索" };
            if ui.button(label).clicked() {
                self.search();
            }
            for n in &self.notes {
                ui.weak(n);
            }
        });
    }

    fn render_rows(&mut self, ui: &mut egui::Ui) {
        if self.rows.is_empty() {
            ui.weak(if self.searched.is_some() { "当たるテキストはありません" } else { "条件を入れて「検索」を押してください" });
            return;
        }
        let mut focus: Option<FocusTarget> = None;
        let scene_id = self.searched_scene;
        let mut last_scene: Option<(String, bool)> = None;
        for r in &mut self.rows {
            let key = (r.scene.clone(), r.handle.is_some());
            if last_scene.as_ref() != Some(&key) {
                ui.add_space(4.0);
                let label = if r.handle.is_some() { format!("▼ {}（表示中・書き換えられる）", r.scene) } else { format!("▼ {}（保存済み・見るだけ）", r.scene) };
                ui.strong(label);
                last_scene = Some(key);
            }
            ui.horizontal(|ui| {
                if r.handle.is_some() {
                    ui.checkbox(&mut r.checked, "");
                } else {
                    ui.add_space(22.0);
                }
                let place = format!("L{} {}-{}", r.layer + 1, r.start, r.end);
                let resp = ui.add(egui::Label::new(egui::RichText::new(place).monospace()).sense(egui::Sense::click()));
                if let (Some(h), Some(scene_id)) = (r.handle, scene_id) {
                    if resp.on_hover_text("クリックでこのオブジェクトにフォーカスを移す").clicked() {
                        focus = Some(FocusTarget {
                            handle: h,
                            scene_id,
                            target: r.target.clone(),
                            raw: r.raw.clone(),
                            layer: r.layer,
                            start: r.start,
                        });
                    }
                }
                let label = r.target.label();
                if !label.is_empty() {
                    ui.weak(label);
                }
                ui.weak(format!("{} 件", r.hits));
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_space(26.0);
                ui.label(text::preview(&r.before, PREVIEW));
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_space(26.0);
                ui.colored_label(if r.handle.is_some() { GREEN } else { YELLOW }, format!("→ {}", text::preview(&r.after, PREVIEW)))
                    .on_hover_text(&r.after);
            });
        }
        // フォーカスはクリックしたときだけ移す（ルール au2-rs-plugin）
        if let Some(t) = focus {
            if let Err(e) = edit_ops::focus(t) {
                self.set_status(e, true);
            }
        }
    }

    fn render_style(&mut self, ui: &mut egui::Ui) {
        ui.label("フォーカス中のテキストの見た目（フォント・色など）を、選択中のテキストへ写します。本文は変えません");
        ui.horizontal(|ui| {
            if ui.button("元を読む").on_hover_text("いまフォーカスしているテキストオブジェクトの設定を読みます").clicked() {
                match edit_ops::read_style_source() {
                    Ok((h, items)) => {
                        let items = items.into_iter().map(|(n, v)| (STYLE_DEFAULTS.contains(&n.as_str()), n, v)).collect();
                        self.style.source = Some((h, items));
                        self.set_status("元を読みました。写す項目を選んでください", false);
                    }
                    Err(e) => self.set_status(e, true),
                }
            }
            if ui.button("選択を読む").on_hover_text("いま選択しているオブジェクトのうち、テキストのものを写し先にします").clicked() {
                match edit_ops::read_selected_texts() {
                    Ok(t) => {
                        self.set_status(format!("選択中のテキスト {} 個を写し先にしました", t.len()), false);
                        self.style.targets = Some(t);
                    }
                    Err(e) => self.set_status(e, true),
                }
            }
        });
        ui.separator();
        let Some((_, items)) = &mut self.style.source else {
            ui.weak("1. 見本にするテキストをフォーカスして「元を読む」 2. 写したいテキストを選択して「選択を読む」 3. 下のボタンで写す");
            return;
        };
        egui::ScrollArea::vertical().id_salt("style").auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("style_grid").num_columns(2).striped(true).show(ui, |ui| {
                for (on, name, value) in items.iter_mut() {
                    ui.checkbox(on, name.as_str());
                    ui.weak(text::preview(value, 40));
                    ui.end_row();
                }
            });
        });
    }

    fn apply_style(&mut self) {
        let (Some((source, items)), Some(targets)) = (&self.style.source, &self.style.targets) else { return };
        let items: Vec<(String, String)> = items.iter().filter(|(on, _, _)| *on).map(|(_, n, v)| (n.clone(), v.clone())).collect();
        let targets: Vec<ObjectHandle> = targets.iter().copied().filter(|t| t != source).collect();
        if items.is_empty() || targets.is_empty() {
            return;
        }
        let (n, k) = (targets.len(), items.len());
        match edit_ops::apply_style(targets, items) {
            Ok(rep) => {
                tracing::info!(
                    "[TextReplace_H] そろえる: {} / {n} 個（{k} 項目、中間点の数が違う {}、失敗 {}）",
                    rep.written,
                    rep.points_mismatch.len(),
                    rep.errors.len()
                );
                let mut msg = format!("{} / {n} 個の {k} 項目をそろえました（Ctrl+Z で取り消せます）", rep.written);
                if !rep.points_mismatch.is_empty() {
                    msg.push_str(&format!(
                        "。中間点の数が違うので写さなかった（移動している項目）: {}",
                        rep.points_mismatch.join("、")
                    ));
                }
                if !rep.errors.is_empty() {
                    msg.push_str(&format!("。失敗: {}", rep.errors.join("、")));
                }
                let error = !rep.points_mismatch.is_empty() || !rep.errors.is_empty();
                self.set_status(msg, error);
            }
            Err(e) => {
                // 選択が変わっていたら、読み直してもらう
                self.style.targets = None;
                self.set_status(e, true);
            }
        }
    }

    fn render_bottom(&mut self, ui: &mut egui::Ui) {
        if self.q.mode == Mode::Style {
            let n_items = self.style.source.as_ref().map(|(_, i)| i.iter().filter(|(on, _, _)| *on).count()).unwrap_or(0);
            let source = self.style.source.as_ref().map(|(h, _)| *h);
            let n = self.style.targets.as_ref().map(|t| t.iter().filter(|h| Some(**h) != source).count()).unwrap_or(0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = ui.add_enabled(n > 0 && n_items > 0, egui::Button::new(format!("選択中の {n} 個に {n_items} 項目を写す")));
                    if b.clicked() {
                        self.apply_style();
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| self.render_status(ui, false));
                });
            });
            return;
        }
        let stale = self.searched.as_ref().is_some_and(|s| s != &self.query_for_compare());
        let n = self.rows.iter().filter(|r| r.checked && r.handle.is_some()).count();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let verb = match self.searched.as_ref().map(|s| s.mode).unwrap_or(self.q.mode) {
                    Mode::Wrap => "改行",
                    _ => "置換",
                };
                let b = ui.add_enabled(n > 0 && !stale, egui::Button::new(format!("チェックした {n} 件を{verb}")));
                let b = if stale { b.on_disabled_hover_text("条件が変わりました。検索し直してください") } else { b };
                if b.clicked() {
                    self.apply();
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| self.render_status(ui, stale));
            });
        });
    }

    /// 検索したときの条件と比べるための、今の条件（辞書のときは今の辞書を入れる）
    fn query_for_compare(&self) -> Query {
        let mut q = self.q.clone();
        if q.mode == Mode::Dictionary {
            q.dict = self.dict.clone();
        }
        q
    }

    fn render_status(&self, ui: &mut egui::Ui, stale: bool) {
        let text = egui::RichText::new(if self.status.is_empty() { " " } else { &self.status }).small();
        if self.status_is_error {
            ui.colored_label(RED, text);
        } else if stale {
            ui.colored_label(YELLOW, egui::RichText::new("条件が変わりました。検索し直してください").small());
        } else {
            ui.label(text);
        }
    }
}

impl eframe::App for TextReplaceApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show(ui, |ui| self.render_top(ui));
        egui::Panel::bottom("bottom").show(ui, |ui| self.render_bottom(ui));
        // 中央パネルは最後に追加する（ルール au2-rs-plugin「egui レイアウト」）
        egui::CentralPanel::default().show(ui, |ui| {
            if self.q.mode == Mode::Style {
                self.render_style(ui);
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.render_rows(ui));
        });
    }
}
