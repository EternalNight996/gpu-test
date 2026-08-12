//! gpu-test — 显卡功能性测试插件（兼容所有显卡，先 NVIDIA；e-autotest 接入）
//!
//! 用法：
//!   gpu-test.exe [--sn SN] [--station ST] [--mode MODE]
//!                    [--samples N] [--auto] [--close SECS]
//!                    [--res PATH] [--no-gui]
//!
//! - 默认：GUI 弹窗，检测完需人工点"确认"关闭
//! - --auto：检测完倒计时自动关闭（--close 控制秒数，默认 5）
//! - --no-gui：不弹窗，命令行直接输出 R<json>R（调试/CI 用）
//! - --res PATH：额外把 R<json>R 写入文件（对应平台 res_url）
//!
//! 输出：stdout 打印一行 R<{json}>R，退出码 0=通过 / 非0=失败。

// 与 heg-os-active2 同款：release 为 GUI 子系统（双击不弹命令行），
// 启动时 reattach_windows_terminal() 挂父控制台，cmd 下也能看到 stdout 日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod detect;
mod gui;
mod logger;

use e_log::preload::*;
use std::io::Write;
use std::process::exit;

const DEFAULT_CLOSE_SECS: u32 = 5;

#[derive(Default, Clone)]
struct Args {
    sn: String,
    station: String,
    mode: String,
    samples: u32,
    info: bool,
    auto: bool,
    close: u32,
    res: String,
    no_gui: bool,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args { samples: detect::DEFAULT_SAMPLES, close: DEFAULT_CLOSE_SECS, ..Default::default() };
    let mut i = 0;
    let take = |i: &mut usize, name: &str| -> Result<String, String> {
        *i += 1;
        argv.get(*i).cloned().ok_or_else(|| format!("缺少参数值: {name}"))
    };
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--sn" => a.sn = take(&mut i, "--sn")?,
            "--station" => a.station = take(&mut i, "--station")?,
            "--mode" => a.mode = take(&mut i, "--mode")?,
            "--samples" => {
                let v = take(&mut i, "--samples")?;
                a.samples = v.parse().map_err(|_| format!("--samples 需要数字: {v}"))?;
            }
            "--close" => {
                let v = take(&mut i, "--close")?;
                a.close = v.parse().map_err(|_| format!("--close 需要数字: {v}"))?;
            }
            "--res" => a.res = take(&mut i, "--res")?,
            "--info" => a.info = true,
            "--auto" => a.auto = true,
            "--no-gui" => a.no_gui = true,
            "--help" | "-h" => {
                print_help();
                exit(0);
            }
            other => return Err(format!("未知参数: {other}")),
        }
        i += 1;
    }
    Ok(a)
}

fn print_help() {
    println!(
        "显卡功能性测试插件（e-autotest 接入）
用法: gpu-test [选项]
  --sn SN        序列号（平台标准参数）
  --station ST   工站（平台标准参数）
  --mode MODE    模式（平台标准参数）
  --samples N    nvidia-smi 稳定性采样次数（默认 3）
  --info         只输出显卡型号标识（GPU: 型号 [10de:xxxx]），供平台 filter 比对首件与量产
  --auto         检测完倒计时自动关闭（默认需人工确认）
  --close SECS   自动关闭倒计时秒数（默认 5）
  --res PATH     额外把 R<json>R 写入文件（对应平台 res_url）
  --no-gui       不弹窗，命令行直接输出（调试/CI）
  --help         显示本帮助"
    );
}

/// 构造 e-autotest R<json>R 字符串
fn build_rlog(content: &str, status: bool) -> String {
    let json = serde_json::json!({
        "content": content,
        "status": status,
        "opts": {
            "api": "None",
            "task": "",
            "init": false,
            "full": false,
            "filter": [],
            "args": [],
            "command": [],
        }
    });
    format!("R<{json}>R\n")
}

fn emit(content: &str, status: bool, res: &str) {
    let rlog = build_rlog(content, status);
    // 文件输出（平台 res_url 方式）
    if !res.is_empty() {
        if let Err(e) = std::fs::write(res, &rlog) {
            eprintln!("写入结果文件失败 {res}: {e}");
        }
    }
    // stdout 输出（console 模式 / 调试；windowed 下可能无 stdout，忽略错误）
    let _ = std::io::stdout().write_all(rlog.as_bytes());
    let _ = std::io::stdout().flush();
}

fn main() -> std::process::ExitCode {
    let _guards = logger::init();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            // 参数错误也要有 R 输出（平台扫描 R 标签）
            e_log::error!(target: "gpu-test", "参数解析失败 - {e}");
            drop(_guards); // 冲刷日志，保证 stdout 先出日志行、R 行收尾
            emit(&format!("FAIL: 参数解析失败 - {e}"), false, "");
            return std::process::ExitCode::FAILURE;
        }
    };
    info!(
        target: "gpu-test",
        "运行开始: sn={} station={} mode={} samples={}",
        args.sn,
        args.station,
        args.mode,
        args.samples,
    );

    // 型号标识接口：只返回显卡型号，不做驱动/稳定性检测，供平台比对
    if args.info {
        let (status, content) = detect::gpu_identities();
        info!(target: "gpu-test", "型号标识:\n{content}");
        drop(_guards);
        emit(&content, status, &args.res);
        return if status {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }

    let (status, content) = if !args.no_gui {
        // GUI 弹窗模式
        let opts = gui::GuiOptions { samples: args.samples, auto: args.auto, close_secs: args.close };
        let result = gui::run_gui(opts);
        (result.status, result.content)
    } else {
        // 命令行模式（--no-gui）
        detect::detect(args.samples, None)
    };
    // 日志：运行头 + 检测明细，结尾附一行 R<...>R 平台格式输出
    info!(
        target: "gpu-test",
        "status={} sn={} station={} mode={} samples={}\n{}",
        if status { "PASS" } else { "FAIL" },
        args.sn,
        args.station,
        args.mode,
        args.samples,
        content,
    );
    info!(target: "gpu-test", "{}", build_rlog(&content, status).trim_end());
    drop(_guards);
    emit(&content, status, &args.res);
    if status {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
