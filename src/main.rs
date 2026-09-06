//! gpu-test — 显卡功能性测试插件（兼容所有显卡，先 NVIDIA；e-autotest 接入）
//!
//! 用法：
//!   gpu-test.exe [--samples N] [--auto] [--close SECS]
//!                    [--res] [--no-gui] [--init-config]
//!
//! - 默认：GUI 弹窗，检测完需人工点"确认"关闭
//! - --auto：检测完倒计时自动关闭（--close 控制秒数，默认 5）
//! - --no-gui：不弹窗，命令行直接输出 R<json>R（调试/CI 用）
//! - --res：额外把 R<json>R 覆盖写入 logs/gpu-test.log（etch 风格，仅留最新一条）
//! - --info：纯文本输出型号标识（不输出日志），供平台 filter 比对首件与量产
//!
//! 配置：与程序同目录放置 gpu-test.toml（[rule]/[run] 分段，参考 etch 命名规范）可提供默认值，
//! 不存在时自动生成一份默认配置；`--init-config` 可随时重新生成；命令行参数优先。
//!
//! 输出：stdout 打印一行 R<{json}>R，退出码 0=通过 / 非0=失败。

// 与 heg-os-active2 同款：release 为 GUI 子系统（双击不弹命令行），
// 启动时 reattach_windows_terminal() 挂父控制台，cmd 下也能看到 stdout 日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod detect;
mod gui;
mod logger;

use e_log::preload::*;
use std::io::Write;
use std::process::exit;

#[derive(Default, Clone)]
struct Args {
    samples: u32,
    info: bool,
    auto: bool,
    close: u32,
    no_gui: bool,
    res: bool,
    gpu_rule: Vec<String>,
    init_config: bool,
}

/// 配置文件（gpu-test.toml）提供默认值，命令行参数在此基础上覆盖
impl From<config::Config> for Args {
    fn from(c: config::Config) -> Self {
        Args {
            samples: c.run.samples,
            info: c.run.info,
            auto: c.run.auto,
            close: c.run.close,
            no_gui: c.run.no_gui,
            res: false,
            gpu_rule: c.rule.gpu,
            init_config: false,
        }
    }
}

fn parse_args(argv: &[String], a: &mut Args) -> Result<(), String> {
    let mut i = 0;
    let take = |i: &mut usize, name: &str| -> Result<String, String> {
        *i += 1;
        argv.get(*i).cloned().ok_or_else(|| format!("缺少参数值: {name}"))
    };
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--samples" => {
                let v = take(&mut i, "--samples")?;
                a.samples = v.parse().map_err(|_| format!("--samples 需要数字: {v}"))?;
            }
            "--close" => {
                let v = take(&mut i, "--close")?;
                a.close = v.parse().map_err(|_| format!("--close 需要数字: {v}"))?;
            }
            "--res" => a.res = true,
            "--info" => a.info = true,
            "--auto" => a.auto = true,
            "--no-gui" => a.no_gui = true,
            "--init-config" => a.init_config = true,
            "--help" | "-h" => {
                print_help();
                exit(0);
            }
            other => return Err(format!("未知参数: {other}")),
        }
        i += 1;
    }
    Ok(())
}

fn print_help() {
    println!(
        "显卡功能性测试插件（e-autotest 接入）
用法: gpu-test [选项]
  --samples N    nvidia-smi 稳定性采样次数（默认 3）
  --info         只输出显卡型号标识（GPU: 型号 [10de:xxxx]），纯文本 R<...>R 发送、不输出日志，供平台 filter 比对
  --auto         检测完倒计时自动关闭（默认需人工确认）
  --close SECS   自动关闭倒计时秒数（默认 5）
  --res          额外把 R<json>R 覆盖写入 logs/gpu-test.log（etch 风格，仅留最新一条）
  --no-gui       不弹窗，命令行直接输出（调试/CI）
  --init-config  重新输出一份 gpu-test.toml（默认配置，编辑后同目录生效）
  --help         显示本帮助

配置: 与程序同目录放置 gpu-test.toml（[rule]/[run] 分段，参考 etch 命名规范）可提供默认值
（不存在时自动生成一份），命令行参数优先。"
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

fn emit(content: &str, status: bool, res: bool) {
    let rlog = build_rlog(content, status);
    // --res：覆盖写 logs/gpu-test.log（etch 风格，只留最新一条 R 结果）
    if res {
        let _ = std::fs::create_dir_all("logs");
        if let Ok(mut f) = std::fs::File::create("logs/gpu-test.log") {
            if let Err(e) = f.write_all(rlog.as_bytes()) {
                eprintln!("写入结果文件 logs/gpu-test.log 失败: {e}");
            }
        }
    }
    // stdout 输出（console 模式 / 调试；windowed 下可能无 stdout，忽略错误）
    let _ = std::io::stdout().write_all(rlog.as_bytes());
    let _ = std::io::stdout().flush();
}

fn main() -> std::process::ExitCode {
    // 日志初始化（文件 + stdout）：reattach 挂接父控制台，保证 release GUI 子系统下 stdout 可见；
    // --info / --init-config 等纯文本分支不打印日志行，stdout 只有 R<...>R / 提示文本
    let _guards = logger::init();
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // 先读配置文件（项目名.toml，如 gpu-test.toml）提供默认值，命令行参数再覆盖
    let mut args = Args::from(config::Config::load());
    if let Err(e) = parse_args(&argv, &mut args) {
        // 参数错误也要有 R 输出（平台扫描 R 标签）
        e_log::error!(target: "gpu-test", "参数解析失败 - {e}");
        drop(_guards); // 冲刷日志，保证 stdout 先出日志行、R 行收尾
        emit(&format!("FAIL: 参数解析失败 - {e}"), false, args.res);
        return std::process::ExitCode::FAILURE;
    }

    // --init-config：输出一份 项目名.toml 默认配置后退出
    if args.init_config {
        return match config::write_default() {
            Ok(path) => {
                info!(target: "gpu-test", "已输出配置文件: {}", path.display());
                drop(_guards);
                println!("已输出配置文件: {}", path.display());
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                e_log::error!(target: "gpu-test", "输出配置文件失败 - {e}");
                drop(_guards);
                emit(&format!("FAIL: 输出配置文件失败 - {e}"), false, args.res);
                std::process::ExitCode::FAILURE
            }
        };
    }

    // --info：纯文本输出型号标识（不打印日志行），供平台 filter 比对首件与量产
    if args.info {
        let (status, content) = detect::gpu_identities();
        drop(_guards);
        emit(&content, status, args.res);
        return if status {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }

    let rule_hint = if args.gpu_rule.is_empty() {
        "不限".to_string()
    } else {
        args.gpu_rule.join(" / ")
    };
    info!(
        target: "gpu-test",
        "运行开始: samples={} 限定匹配={}",
        args.samples,
        rule_hint,
    );

    let (status, content) = if !args.no_gui {
        // GUI 弹窗模式
        let opts = gui::GuiOptions {
            samples: args.samples,
            auto: args.auto,
            close_secs: args.close,
            gpu_rule: args.gpu_rule.clone(),
        };
        let result = gui::run_gui(opts);
        (result.status, result.content)
    } else {
        // 命令行模式（--no-gui）
        detect::detect(args.samples, &args.gpu_rule, None)
    };
    // 日志：运行头 + 检测明细，结尾附一行 R<...>R 平台格式输出
    info!(
        target: "gpu-test",
        "status={} samples={} 限定匹配={}\n{}",
        if status { "PASS" } else { "FAIL" },
        args.samples,
        rule_hint,
        content,
    );
    info!(target: "gpu-test", "{}", build_rlog(&content, status).trim_end());
    drop(_guards);
    emit(&content, status, args.res);
    if status {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
