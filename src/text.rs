//! テキストの本文の処理（本体と関係なく、cargo test の中心）。
//!
//! - 保存形式との変換（`unescape` / `escape`）: 本体は改行を `\n` の 2 文字、`\` を `\\` で返す
//!   （実機で確認。`AI/docs/20261008_パラメータ編集_SDK実機確認.md`）
//! - 制御文字（`<#色>` `<s32>` `<$プリセット>` など。`aviutl2.txt` の「テキスト」）と地の文の切り分け
//! - 検索・置換（地の文だけ。大文字小文字・全角半角・正規表現）
//! - 指定した文字数での改行

use std::ops::Range;

use unicode_normalization::UnicodeNormalization;

/// 本体が返す形（`\n` = 改行、`\\` = `\`）を本文に戻す。ほかの `\` はそのまま
pub fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.peek() {
                Some('n') => {
                    it.next();
                    out.push('\n');
                }
                Some('\\') => {
                    it.next();
                    out.push('\\');
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 本文を本体へ書く形にする（`\` → `\\`、改行 → `\n`、CR は落とす）
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// 本文を制御文字（true）と地の文（false）の範囲に分ける。
/// 制御文字は `<` から最初の `>` まで。ただし `<//…//>`（コメント）と `<?…?>`（スクリプト）はその終わりまで。閉じない `<` は地の文
pub fn segments(text: &str) -> Vec<(Range<usize>, bool)> {
    let mut out = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with('<') {
            let rest = &text[i..];
            let close = if rest.starts_with("<//") {
                rest[3..].find("//>").map(|p| 3 + p + 3)
            } else if rest.starts_with("<?") {
                rest[2..].find("?>").map(|p| 2 + p + 2)
            } else {
                rest.find('>').map(|p| p + 1)
            };
            if let Some(len) = close {
                if plain_start < i {
                    out.push((plain_start..i, false));
                }
                out.push((i..i + len, true));
                i += len;
                plain_start = i;
                continue;
            }
        }
        i += text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
    if plain_start < text.len() {
        out.push((plain_start..text.len(), false));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub case_sensitive: bool,
    /// 全角と半角を同じとみなす（NFKC。半角カナの濁点も合わせる）。正規表現のときは使わない
    pub fold_width: bool,
    pub regex: bool,
    /// 制御文字の中は探さない
    pub skip_tags: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { case_sensitive: false, fold_width: false, regex: false, skip_tags: true }
    }
}

/// 比べるための形にした文字と、元の本文の範囲
fn fold(text: &str, base: usize, opts: &Options) -> Vec<(char, Range<usize>)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let pos = chars[i].0;
        // 半角カナ＋半角の濁点・半濁点はまとめて 1 文字に
        let mut take = 1;
        if opts.fold_width && i + 1 < chars.len() && matches!(chars[i + 1].1, '\u{FF9E}' | '\u{FF9F}') {
            take = 2;
        }
        let end = if i + take < chars.len() { chars[i + take].0 } else { text.len() };
        let src = &text[pos..end];
        let folded: String = if opts.fold_width { src.nfkc().collect() } else { src.to_string() };
        for f in folded.chars() {
            if opts.case_sensitive {
                out.push((f, base + pos..base + end));
            } else {
                for l in f.to_lowercase() {
                    out.push((l, base + pos..base + end));
                }
            }
        }
        i += take;
    }
    out
}

/// 1 つの範囲（地の文、または本文全体）の中で探す
fn find_in(text: &str, range: Range<usize>, query: &str, opts: &Options, re: Option<&regex::Regex>) -> Vec<Range<usize>> {
    let part = &text[range.clone()];
    if let Some(re) = re {
        return re.find_iter(part).filter(|m| !m.as_str().is_empty()).map(|m| range.start + m.start()..range.start + m.end()).collect();
    }
    let hay = fold(part, range.start, opts);
    let needle: Vec<char> = fold(query, 0, opts).into_iter().map(|(c, _)| c).collect();
    let mut out = Vec::new();
    if needle.is_empty() || hay.len() < needle.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()].iter().map(|(c, _)| *c).eq(needle.iter().copied()) {
            let (s, e) = (hay[i].1.start, hay[i + needle.len() - 1].1.end);
            // 合成した文字の途中で始まる・終わる当たりは捨てる
            if out.last().is_none_or(|l: &Range<usize>| l.end <= s) {
                out.push(s..e);
            }
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

fn build_regex(query: &str, opts: &Options) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(query)
        .case_insensitive(!opts.case_sensitive)
        .build()
        .map_err(|e| format!("正規表現が読めません: {e}"))
}

/// 当たった範囲（本文のバイト位置）
pub fn find(text: &str, query: &str, opts: &Options) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let re = if opts.regex { Some(build_regex(query, opts)?) } else { None };
    if !opts.skip_tags {
        return Ok(find_in(text, 0..text.len(), query, opts, re.as_ref()));
    }
    Ok(segments(text)
        .into_iter()
        .filter(|(_, tag)| !tag)
        .flat_map(|(r, _)| find_in(text, r, query, opts, re.as_ref()))
        .collect())
}

/// 置き換える。正規表現のときは置換語の `$1` などが使える。戻り値は（新しい本文、置き換えた数）
pub fn replace(text: &str, query: &str, replacement: &str, opts: &Options) -> Result<(String, usize), String> {
    let hits = find(text, query, opts)?;
    if hits.is_empty() {
        return Ok((text.to_string(), 0));
    }
    let re = if opts.regex { Some(build_regex(query, opts)?) } else { None };
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for r in &hits {
        out.push_str(&text[last..r.start]);
        match &re {
            Some(re) => {
                let m = &text[r.clone()];
                out.push_str(&re.replace(m, replacement));
            }
            None => out.push_str(replacement),
        }
        last = r.end;
    }
    out.push_str(&text[last..]);
    Ok((out, hits.len()))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WrapOptions {
    /// 1 行の文字数
    pub width: f64,
    /// 半角の文字を 0.5 文字と数える
    pub half_as_half: bool,
    /// 今の改行を残す（残さないなら消してから折り返す）
    pub keep_newlines: bool,
    /// 行頭禁則（句読点や閉じ括弧で行を始めない。その行が 1 文字はみ出す）
    pub kinsoku: bool,
}

impl Default for WrapOptions {
    fn default() -> Self {
        WrapOptions { width: 16.0, half_as_half: true, keep_newlines: true, kinsoku: true }
    }
}

fn is_half(c: char) -> bool {
    (' '..='~').contains(&c) || ('\u{FF61}'..='\u{FF9F}').contains(&c)
}

/// 行の頭に置かない文字
fn no_line_start(c: char) -> bool {
    "、。，．,.・：；？！?!ー―…‥」』）)］]｝}〕〉》】〙〗ゝゞヽヾ々ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ".contains(c)
}

/// 指定した文字数で改行する。制御文字は数えず、途中で切らない
pub fn wrap(text: &str, o: &WrapOptions) -> String {
    let width = o.width.max(0.5);
    let mut out = String::with_capacity(text.len() + 16);
    let mut line = 0.0f64;
    for (r, tag) in segments(text) {
        let part = &text[r];
        if tag {
            out.push_str(part);
            continue;
        }
        for c in part.chars() {
            match c {
                '\r' => {}
                '\n' if o.keep_newlines => {
                    out.push('\n');
                    line = 0.0;
                }
                '\n' => {}
                c => {
                    let w = if o.half_as_half && is_half(c) { 0.5 } else { 1.0 };
                    if line > 0.0 && line + w > width + 1e-9 && !(o.kinsoku && no_line_start(c)) {
                        out.push('\n');
                        line = 0.0;
                    }
                    out.push(c);
                    line += w;
                }
            }
        }
    }
    out
}

/// 一覧に出す短い形（改行を ⏎ にし、長ければ切る）
pub fn preview(text: &str, max_chars: usize) -> String {
    let s: String = text.chars().map(|c| if c == '\n' { '⏎' } else { c }).collect();
    if s.chars().count() > max_chars {
        let mut t: String = s.chars().take(max_chars).collect();
        t.push('…');
        t
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_roundtrip() {
        // 実機で読んだ形（中間点プローブ_H の T7）
        assert_eq!(unescape("line1\\nline2"), "line1\nline2");
        assert_eq!(unescape("back\\\\slash"), "back\\slash");
        assert_eq!(unescape("a\\b"), "a\\b");
        for t in ["line1\nline2", "back\\slash", "a\\nb は \\ と n", "", "<#ff0000>赤"] {
            assert_eq!(unescape(&escape(t)), t);
        }
        assert_eq!(escape("a\r\nb"), "a\\nb");
    }

    #[test]
    fn tag_segments() {
        let t = "前<#ff0000>赤<s*1.5>大<//コメント > を含む//>後<?=obj.time?>終<未閉";
        let tags: Vec<&str> = segments(t).into_iter().filter(|(_, tag)| *tag).map(|(r, _)| &t[r]).collect();
        assert_eq!(tags, vec!["<#ff0000>", "<s*1.5>", "<//コメント > を含む//>", "<?=obj.time?>"]);
        let plain: String = segments(t).into_iter().filter(|(_, tag)| !tag).map(|(r, _)| &t[r]).collect();
        assert_eq!(plain, "前赤大後終<未閉");
    }

    #[test]
    fn search_options() {
        let o = Options::default();
        // 制御文字の中（<$強調> の「強調」）には当たらない
        assert_eq!(find("<$強調>強調", "強調", &o).unwrap().len(), 1);
        assert_eq!(find("<$強調>強調", "強調", &Options { skip_tags: false, ..o }).unwrap().len(), 2);
        // 大文字小文字
        assert_eq!(find("Uma uma", "UMA", &o).unwrap().len(), 2);
        assert_eq!(find("Uma uma", "UMA", &Options { case_sensitive: true, ..o }).unwrap().len(), 0);
        // 全角半角（半角カナの濁点を含む）
        let w = Options { fold_width: true, ..o };
        assert_eq!(find("ｳﾏ娘とウマ娘とＵＭＡ", "ウマ娘", &w).unwrap().len(), 2);
        assert_eq!(find("ｶﾞｲﾄﾞ", "ガイド", &w).unwrap(), vec![0..15]);
        assert_eq!(find("ｶﾞｲﾄﾞ", "ガイド", &o).unwrap().len(), 0);
        // 正規表現と置換の $1
        let r = Options { regex: true, ..o };
        assert_eq!(replace("第1話と第23話", r"第(\d+)話", "Ep.$1", &r).unwrap(), ("Ep.1とEp.23".to_string(), 2));
        assert!(find("x", "(", &r).is_err());
    }

    #[test]
    fn replace_keeps_tags() {
        let o = Options { fold_width: true, ..Options::default() };
        let t = "…が<#2B1B4A>ｳﾏ娘<#>の<$ウマ娘>";
        let (n, c) = replace(t, "ウマ娘", "ウマ娘", &o).unwrap();
        assert_eq!(c, 1);
        assert_eq!(n, "…が<#2B1B4A>ウマ娘<#>の<$ウマ娘>");
    }

    #[test]
    fn wrap_by_chars() {
        let o = WrapOptions { width: 5.0, half_as_half: false, keep_newlines: true, kinsoku: false };
        assert_eq!(wrap("あいうえおかきくけこさ", &o), "あいうえお\nかきくけこ\nさ");
        // 今の改行で数え直す
        assert_eq!(wrap("あい\nうえおかきく", &o), "あい\nうえおかき\nく");
        // 今の改行を消してから
        assert_eq!(wrap("あい\nうえおかきく", &WrapOptions { keep_newlines: false, ..o }), "あいうえお\nかきく");
        // 制御文字は数えず、切らない
        assert_eq!(wrap("あいう<#ff0000>えおか<#>き", &o), "あいう<#ff0000>えお\nか<#>き");
        // 半角は 0.5
        assert_eq!(wrap("abcdefghijkl", &WrapOptions { half_as_half: true, ..o }), "abcdefghij\nkl");
        // 行頭禁則: 句点を次の行の頭にしない
        assert_eq!(wrap("あいうえお。かき", &WrapOptions { kinsoku: true, ..o }), "あいうえお。\nかき");
        // ちょうど割り切れるとき、最後に改行を足さない
        assert_eq!(wrap("あいうえお", &o), "あいうえお");
        assert_eq!(preview("a\nb", 10), "a⏎b");
    }
}
