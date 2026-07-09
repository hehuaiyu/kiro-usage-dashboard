//! kiro-core: Kiro 用量数据层（扫描 / 持久化 / 聚合），UI 无关。
//!
//! 被两个前端复用：
//!   - src-tauri  (WebView2 版)
//!   - slint-app  (Slint 版, 无 GPU 机器友好)
//!
//! 模块内部的 `crate::models` / `crate::util` 等引用在本 crate 里依然有效，
//! 所以从 src-tauri 移过来的文件几乎不用改。

pub mod history_store;
pub mod models;
pub mod quota_snapshot;
pub mod scanner;
pub mod util;
