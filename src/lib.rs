//! TextReplace_H（テキスト置換）
//!
//! テキストオブジェクトの本文を検索し、表示中のシーンはまとめて置換する。指定した文字数で改行することもできる。
//! ほかのシーンは保存済みの `.aup2` から「どこにあるか」を出すだけ。
//! 仕様は `AI/specifications/20261005_TextReplace_H_spec.md`。コミュニティの要望「高度なパラメータ編集プラグイン」の
//! 「高度なテキスト編集（指定文字数で改行や置換など）」に当たる。
//!
//! 第 2 段階: 置換の辞書（`dict.rs`）、第三者スクリプトの文字列の項目、見た目（フォント・色など）をそろえる。
//!
//! 自分で起こすスレッドは無い。

mod aup2;
mod dict;
mod edit_ops;
mod gui;
pub mod text;

use std::path::PathBuf;
use std::sync::Arc;

use aviutl2::generic::GlobalEditHandle;
use aviutl2::AnyResult;
use parking_lot::{Mutex, RwLock};

pub const WINDOW_NAME: &str = "テキスト置換";

pub(crate) static EDIT_HANDLE: GlobalEditHandle = GlobalEditHandle::new();

fn init_logging() {
    use aviutl2::tracing::Level;
    let level = if cfg!(debug_assertions) { Level::DEBUG } else { Level::INFO };
    let _ = aviutl2::tracing_subscriber::fmt()
        .with_max_level(level)
        .event_format(aviutl2::logger::AviUtl2Formatter)
        .with_writer(aviutl2::logger::AviUtl2LogWriter)
        .try_init();
}

#[aviutl2::plugin(GenericPlugin)]
pub struct TextReplacePlugin {
    window: Mutex<Option<aviutl2_eframe::EframeWindow>>,
    /// 開いているプロジェクトの `.aup2`（ほかのシーンを読むため）。未保存なら None
    project: Arc<RwLock<Option<PathBuf>>>,
}

impl aviutl2::generic::GenericPlugin for TextReplacePlugin {
    fn new(_info: aviutl2::AviUtl2Info) -> AnyResult<Self> {
        init_logging();
        tracing::info!("TextReplace_H v{} 初期化", env!("CARGO_PKG_VERSION"));
        Ok(Self { window: Mutex::new(None), project: Arc::new(RwLock::new(None)) })
    }

    fn plugin_info(&self) -> aviutl2::generic::GenericPluginTable {
        aviutl2::generic::GenericPluginTable {
            name: "TextReplace_H".to_string(),
            information: format!("TextReplace_H v{} - テキスト置換 / by HexBrowns", env!("CARGO_PKG_VERSION")),
        }
    }

    fn register(&mut self, registry: &mut aviutl2::generic::HostAppHandle) {
        EDIT_HANDLE.init(registry.create_edit_handle());
        let project = Arc::clone(&self.project);
        let window = match aviutl2_eframe::EframeWindow::new(WINDOW_NAME, move |cc, handle| {
            Ok(Box::new(gui::TextReplaceApp::new(cc, handle, project)))
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("テキスト置換のウィンドウを作れませんでした: {e:#}");
                return;
            }
        };
        match window.handle() {
            Ok(handle) => {
                if let Err(e) = registry.register_window_client(WINDOW_NAME, &handle) {
                    tracing::error!("register_window_client 失敗: {e}");
                }
            }
            Err(e) => tracing::error!("テキスト置換のウィンドウのハンドルを取れませんでした: {e:#}"),
        }
        *self.window.lock() = Some(window);

        // ウィンドウは表示メニューから開く（メニューから ShowWindow しても開かない。2026-10-10 にユーザーの指摘で外した）
    }

    fn on_project_load(&mut self, project: &mut aviutl2::generic::ProjectFile) {
        *self.project.write() = project.get_path();
    }

    fn on_project_save(&mut self, project: &mut aviutl2::generic::ProjectFile) {
        if let Some(p) = project.get_path() {
            *self.project.write() = Some(p);
        }
    }
}

// 本体が呼ぶ関数（GetCommonPluginTable など）を書き出す（ルール au2-rs-plugin「ビルド」）
aviutl2::register_generic_plugin!(TextReplacePlugin);
