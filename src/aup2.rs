//! 保存済みの `.aup2` から、テキストオブジェクトの本文を読む（表示中でないシーン用。見るだけ）。
//! 読み方は ColorHistory_H の `replace.rs` の `parse_aup2` を写して広げたもの（共有クレートにはしない）。

use std::collections::HashMap;

/// テキストオブジェクト 1 つ
#[derive(Debug, Clone, PartialEq)]
pub struct SavedText {
    pub scene: i32,
    pub layer: usize,
    pub start: usize,
    pub end: usize,
    /// 保存形式のまま（`\n` は 2 文字）
    pub raw: String,
}

/// オブジェクト 1 つの効果の列（第三者スクリプトの文字列の項目を拾うため。(効果名, 同名の何番目, 行)）
#[derive(Debug, Clone, PartialEq)]
pub struct SavedObject {
    pub scene: i32,
    pub layer: usize,
    pub start: usize,
    pub end: usize,
    pub effects: Vec<(String, usize, Vec<(String, String)>)>,
}

#[derive(Debug, Default)]
pub struct Saved {
    pub scenes: HashMap<i32, String>,
    /// 本体のテキストの本文
    pub texts: Vec<SavedText>,
    pub objects: Vec<SavedObject>,
}

pub fn parse(text: &str) -> Saved {
    enum Section {
        Other,
        Scene(i32),
        Object(usize),
        Effect(usize),
    }
    struct Obj {
        scene: i32,
        layer: usize,
        start: usize,
        end: usize,
        /// 今の効果の名前と、テキスト効果なら本文
        effect: String,
        raw: Option<String>,
        effects: Vec<(String, usize, Vec<(String, String)>)>,
    }
    let mut saved = Saved::default();
    let mut objects: Vec<Obj> = Vec::new();
    let mut index_of: HashMap<usize, usize> = HashMap::new();
    let mut section = Section::Other;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = if let Some(n) = name.strip_prefix("scene.") {
                n.parse().map(Section::Scene).unwrap_or(Section::Other)
            } else if let Some((obj, eff)) = name.split_once('.') {
                match (obj.parse::<usize>(), eff.parse::<usize>()) {
                    (Ok(o), Ok(_)) if index_of.contains_key(&o) => {
                        let idx = index_of[&o];
                        objects[idx].effect.clear();
                        objects[idx].effects.push((String::new(), 0, Vec::new()));
                        Section::Effect(idx)
                    }
                    _ => Section::Other,
                }
            } else if let Ok(o) = name.parse::<usize>() {
                index_of.insert(o, objects.len());
                objects.push(Obj { scene: 0, layer: 0, start: 0, end: 0, effect: String::new(), raw: None, effects: Vec::new() });
                Section::Object(objects.len() - 1)
            } else {
                Section::Other
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match section {
            Section::Scene(id) if key == "name" => {
                saved.scenes.insert(id, value.to_string());
            }
            Section::Object(idx) => {
                let o = &mut objects[idx];
                match key {
                    "layer" => o.layer = value.parse().unwrap_or(0),
                    "scene" => o.scene = value.parse().unwrap_or(0),
                    "frame" => {
                        let f: Vec<usize> = value.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                        o.start = f.first().copied().unwrap_or(0);
                        o.end = f.last().copied().unwrap_or(0);
                    }
                    _ => {}
                }
            }
            Section::Effect(idx) => {
                let o = &mut objects[idx];
                if key == "effect.name" {
                    o.effect = value.to_string();
                    // 同名の効果の何番目か
                    let n = o.effects.iter().filter(|e| e.0 == value).count();
                    if let Some(e) = o.effects.last_mut() {
                        e.0 = value.to_string();
                        e.1 = n;
                    }
                } else {
                    if o.effect == "テキスト" && key == "テキスト" && o.raw.is_none() {
                        o.raw = Some(value.to_string());
                    }
                    if let Some(e) = o.effects.last_mut() {
                        e.2.push((key.to_string(), value.to_string()));
                    }
                }
            }
            _ => {}
        }
    }
    for o in objects {
        if let Some(raw) = o.raw {
            saved.texts.push(SavedText { scene: o.scene, layer: o.layer, start: o.start, end: o.end, raw });
        }
        saved.objects.push(SavedObject { scene: o.scene, layer: o.layer, start: o.start, end: o.end, effects: o.effects });
    }
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_objects() {
        let t = "[project]\nversion=2011000\n[scene.0]\nname=Root\n[scene.1]\nname=Scene2\n\
                 [0]\nlayer=3\nframe=120,239\n[0.0]\neffect.name=テキスト\nサイズ=80.00\nテキスト=言葉が\\n流れる\n[0.1]\neffect.name=標準描画\nX=0.00\n\
                 [1]\nlayer=1\nscene=1\nframe=0,59\n[1.0]\neffect.name=図形\n[1.1]\neffect.name=標準描画\n\
                 [2]\nlayer=5\nscene=1\nframe=60,89\n[2.0]\neffect.name=テキスト\nテキスト=二つ目\n";
        let s = parse(t);
        assert_eq!(s.scenes.get(&1).map(String::as_str), Some("Scene2"));
        assert_eq!(s.texts.len(), 2);
        assert_eq!(s.texts[0], SavedText { scene: 0, layer: 3, start: 120, end: 239, raw: "言葉が\\n流れる".into() });
        assert_eq!((s.texts[1].scene, s.texts[1].layer, s.texts[1].raw.as_str()), (1, 5, "二つ目"));
        // 効果の列（同名の何番目かも数える）
        assert_eq!(s.objects.len(), 3);
        let names: Vec<(&str, usize)> = s.objects[0].effects.iter().map(|e| (e.0.as_str(), e.1)).collect();
        assert_eq!(names, [("テキスト", 0), ("標準描画", 0)]);
        assert_eq!(s.objects[0].effects[0].2[0], ("サイズ".to_string(), "80.00".to_string()));
    }

    #[test]
    fn counts_same_named_effects() {
        let t = "[0]\nlayer=0\nframe=0,9\n[0.0]\neffect.name=縁取り文字@X\n文字=あ\n[0.1]\neffect.name=縁取り文字@X\n文字=い\n";
        let s = parse(t);
        let e = &s.objects[0].effects;
        assert_eq!((e[1].0.as_str(), e[1].1, e[1].2[0].1.as_str()), ("縁取り文字@X", 1, "い"));
    }

    /// 本番の自動バックアップを読めるか（`cargo test -- --ignored reads_real_backup`）
    #[test]
    #[ignore]
    fn reads_real_backup() {
        let dir = std::path::Path::new("C:/ProgramData/aviutl2/Backup");
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        let last = files.iter().rev().find(|p| p.extension().is_some_and(|x| x == "aup2")).unwrap();
        let s = parse(&String::from_utf8_lossy(&std::fs::read(last).unwrap()));
        println!("{} / シーン {} / テキスト {}", last.display(), s.scenes.len(), s.texts.len());
        assert!(!s.texts.is_empty());
    }
}
