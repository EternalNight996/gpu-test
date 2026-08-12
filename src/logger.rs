//! e-log 日志初始化（与 e-autotest/etest 生态同一套框架）。
//!
//! 同时输出到文件 logs/gpu-test.log（非阻塞，追加不滚动）与 stdout（e-log 格式），
//! 便于终端查看与后续按日志格式筛选；R<...>R 结果行仍由主程序单独输出供平台解析。

use e_log::appender::{non_blocking::WorkerGuard, rolling};
use e_log::subscriber::{fmt, layer::SubscriberExt, Registry};
use e_log::{FileShare, Level};

/// 初始化日志（文件 + stdout），返回需要保持存活到进程结束的 WriterGuard。
pub fn init() -> Vec<WorkerGuard> {
    // GUI 子系统（release）从 cmd 启动时挂父控制台，使 stdout 日志可见（heg-os-active2 同款）
    e_log::panic::reattach_windows_terminal();
    let folder = std::path::Path::new("logs");
    let _ = std::fs::create_dir_all(folder);
    let roll = rolling::never(folder, "gpu-test.log", FileShare::Read);
    let (file_writer, file_guard) = e_log::appender::non_blocking(roll);
    let file_layer = fmt::layer().with_ansi(false).with_writer(file_writer);
    let (stdout_writer, stdout_guard) = e_log::appender::non_blocking(std::io::stdout());
    let stdout_layer = fmt::layer().with_ansi(false).with_writer(stdout_writer);
    let sub = Registry::default()
        .with(Level::Info.to_level_filter())
        .with(stdout_layer)
        .with(file_layer);
    e_log::init_subscriber(sub, false);
    vec![file_guard, stdout_guard]
}
