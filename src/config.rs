//! 配置文件支持（项目名.toml，即 gpu-test.toml）
//!
//! 配置文件与命令行参数一一对应，提供默认值；
//! 命令行参数优先，覆盖配置文件中的同名项。
//! 配置不存在时自动生成一份默认配置文件；也可用 `gpu-test --init-config` 随时重新生成。

use serde::{Deserialize, Serialize};

use crate::detect;

/// 自动关闭倒计时默认秒数（与 CLI --close 默认一致）
pub const DEFAULT_CLOSE_SECS: u32 = 5;

fn default_samples() -> u32 {
    detect::DEFAULT_SAMPLES
}
fn default_close() -> u32 {
    DEFAULT_CLOSE_SECS
}

/// 配置文件路径：<项目名>.toml（如 gpu-test.toml），在当前工作目录读取
pub fn file_name() -> String {
    format!("{}.toml", env!("CARGO_PKG_NAME"))
}

/// 与命令行参数一一对应的配置项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub sn: String,
    #[serde(default)]
    pub station: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default = "default_samples")]
    pub samples: u32,
    #[serde(default)]
    pub info: bool,
    #[serde(default)]
    pub auto: bool,
    #[serde(default = "default_close")]
    pub close: u32,
    #[serde(default)]
    pub res: String,
    #[serde(default)]
    pub no_gui: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sn: String::new(),
            station: String::new(),
            mode: String::new(),
            samples: default_samples(),
            info: false,
            auto: false,
            close: default_close(),
            res: String::new(),
            no_gui: false,
        }
    }
}

impl Config {
    /// 读取配置文件；不存在时自动生成一份默认配置，解析失败时回退默认值并告警
    pub fn load() -> Self {
        let path = file_name();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 配置不存在：自动生成一份默认配置（目录只读等写失败时仅告警，不影响运行）
                match write_default() {
                    Ok(p) => e_log::warn!(
                        target: "gpu-test",
                        "未找到配置文件，已自动生成默认配置: {}",
                        p.display(),
                    ),
                    Err(we) => e_log::warn!(
                        target: "gpu-test",
                        "未找到配置文件，且自动生成默认配置失败（仅告警）: {we}",
                    ),
                }
                return Self::default();
            }
            Err(e) => {
                eprintln!("读取配置文件 {path} 失败，使用默认配置: {e}");
                return Self::default();
            }
        };
        match toml::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("解析配置文件 {path} 失败，使用默认配置: {e}");
                Self::default()
            }
        }
    }
}

/// 默认配置文件的完整内容（含注释），`--init-config` 输出与仓库内示例共用
pub const SAMPLE_TOML: &str = "\
# gpu-test 配置文件（与命令行参数一一对应）
# 与程序同目录（当前工作目录）放置后自动读取；命令行参数优先，覆盖同名项。
# 可用 `gpu-test --init-config` 重新生成本文件。

# 序列号（--sn，平台标准参数）
sn = \"\"
# 工站（--station，平台标准参数）
station = \"\"
# 模式（--mode，平台标准参数）
mode = \"\"
# nvidia-smi 稳定性采样次数（--samples，默认 3）
samples = 3
# 只输出显卡型号标识，供平台 filter 比对（--info）
info = false
# 检测完倒计时自动关闭（--auto）
auto = false
# 自动关闭倒计时秒数（--close，默认 5）
close = 5
# 额外把 R<json>R 写入文件（--res PATH）
res = \"\"
# 不弹窗，命令行直接输出（--no-gui）
no_gui = false
";

/// 输出一份 <项目名>.toml 默认配置到当前工作目录，返回生成的文件路径
pub fn write_default() -> std::io::Result<std::path::PathBuf> {
    let path = std::path::PathBuf::from(file_name());
    std::fs::write(&path, SAMPLE_TOML)?;
    Ok(path)
}
