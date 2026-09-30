//! 显卡功能性测试核心逻辑（兼容所有显卡，先 NVIDIA；Windows + Linux 双平台）
//!
//! 判定规则：
//! - 枚举不到任何显卡控制器 -> FAIL（PCI/设备枚举异常）
//! - 仅核显、无 NVIDIA 独显   -> PASS（不误杀核显机型）
//! - 检测到 NVIDIA 显卡：
//!     * nvidia-smi 不可用 / 驱动异常 -> FAIL
//!     * nvidia-smi 连续采样 N 次任一次失败 -> FAIL（概率性丢失）
//!     * 全部通过 -> PASS
//!
//! 平台差异（cfg(target_os)）：
//! - Windows: PowerShell Get-CimInstance Win32_VideoController 枚举 + nvidia-smi
//! - Linux:   lspci -nn 枚举 + nvidia-smi + 内核模块检查

use e_log::preload::*;
use serde::Deserialize;
use std::process::Command;

/// 稳定性采样默认次数
pub const DEFAULT_SAMPLES: u32 = 3;
/// 采样间隔（秒），暴露间歇性/概率性故障
pub const SAMPLE_INTERVAL: u64 = 500; // ms

/// 检测步骤（GUI 用它逐项刷新）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Enum,
    Driver,
    Func,
    Sample,
}

/// 进度回调：stage + 是否完成 + 明细
pub type ProgressCb = Box<dyn Fn(Stage, Option<bool>, String) + Send>;

/// 单张显卡信息（对应 PowerShell Win32_VideoController 的 PascalCase 字段）
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Gpu {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    /// PNPDeviceID（PowerShell 字段名不是 PnpDeviceId，需单独映射）
    #[serde(default, rename = "PNPDeviceID")]
    pub pnp: String,
    /// DriverVersion（PowerShell 字段名不是 Driver，需单独映射）
    #[serde(default, rename = "DriverVersion")]
    pub driver: String,
    /// AdapterRAM（字节；Windows WMI 字段，Linux 无）
    #[serde(default, rename = "AdapterRAM")]
    pub adapter_ram: Option<u64>,
}

/// 执行命令并捕获 stdout（utf-8 lossy）。
/// Windows 下隐藏控制台黑框（CREATE_NO_WINDOW）。
fn run_cmd(prog: &str, args: &[&str]) -> (i32, String) {
    let mut cmd = Command::new(prog);
    cmd.args(args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.output() {
        Ok(out) => {
            let code = out.status.code().unwrap_or(-1);
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            (code, text)
        }
        Err(_) => (-1, String::new()),
    }
}

/// 统一把 PowerShell ConvertTo-Json 输出包成数组（单对象无 []）
fn normalize_vc_json(text: &str) -> String {
    if text.trim_start().starts_with('[') {
        text.to_string()
    } else {
        format!("[{text}]")
    }
}

/// Windows: PowerShell 枚举显卡控制器
#[cfg(target_os = "windows")]
fn enum_gpus() -> (i32, Vec<Gpu>) {
    let (code, out) = run_cmd(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_VideoController | Select-Object Name,Status,PNPDeviceID,DriverVersion,AdapterRAM | ConvertTo-Json -Compress",
        ],
    );
    if code != 0 {
        return (code, Vec::new());
    }
    let gpus: Vec<Gpu> = serde_json::from_str(&normalize_vc_json(&out)).unwrap_or_default();
    (0, gpus)
}

/// Linux: lspci 枚举显卡控制器（兼容 -nn 与裸输出）
#[cfg(target_os = "linux")]
fn enum_gpus() -> (i32, Vec<Gpu>) {
    let (code, out) = run_cmd("lspci", &["-nn"]);
    if code != 0 {
        return (code, Vec::new());
    }
    let mut gpus = Vec::new();
    for line in out.lines() {
        if !(line.contains("VGA compatible controller")
            || line.contains("3D controller")
            || line.contains("Display controller"))
        {
            continue;
        }
        // 01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA106 [GeForce RTX 3060] [10de:2503] (rev a1)
        let gpu = Gpu {
            name: line.to_string(),
            status: String::new(),
            pnp: String::new(),
            driver: String::new(),
            adapter_ram: None,
        };
        gpus.push(gpu);
    }
    (0, gpus)
}

/// 判断是否为 NVIDIA（名称含 NVIDIA 或 PCI vendor id 10DE）
fn is_nvidia(gpu: &Gpu) -> bool {
    let name = gpu.name.to_uppercase();
    let pnp = gpu.pnp.to_uppercase();
    name.contains("NVIDIA") || pnp.contains("10DE") || name.contains("10DE")
}

/// Windows: AdapterRAM 字节 -> "12288 MiB"（<1MiB 视为无效）
#[cfg(target_os = "windows")]
fn vram_mib(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) if b >= 1048576 => format!("{} MiB", b / 1048576),
        _ => String::new(),
    }
}

/// 解析 nvidia-smi csv 输出（每行按 ", " 拆分）
fn parse_smi_csv(out: &str) -> Vec<Vec<String>> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split(", ").map(|s| s.trim().to_string()).collect())
        .collect()
}

/// 读取 nvidia-smi 详情 CSV（driver_version,memory.total,vbios_version），顺序对应 NVIDIA 卡
fn nvidia_smi_details() -> Vec<Vec<String>> {
    let (code, out) = run_cmd(
        "nvidia-smi",
        &["--query-gpu=driver_version,memory.total,vbios_version", "--format=csv,noheader"],
    );
    if code != 0 {
        return Vec::new();
    }
    parse_smi_csv(&out)
}

/// nvidia-smi 功能自检查询（利用率/功耗/温度/时钟）
fn nvidia_smi_stats() -> (i32, String) {
    run_cmd(
        "nvidia-smi",
        &[
            "--query-gpu=name,utilization.gpu,power.draw,temperature.gpu,clocks.sm,clocks.mem",
            "--format=csv,noheader",
        ],
    )
}

/// "PCI\VEN_10DE&DEV_2487" -> "10de:2487"
fn pci_id_from_pnp(pnp: &str) -> Option<String> {
    let mut ven = None;
    let mut dev = None;
    for part in pnp.split('&') {
        let p = part.trim();
        if let Some(i) = p.find("VEN_") {
            ven = Some(p[i + 4..].to_lowercase());
        } else if let Some(i) = p.find("DEV_") {
            dev = Some(p[i + 4..].to_lowercase());
        }
    }
    match (ven, dev) {
        (Some(v), Some(d)) => Some(format!("{v}:{d}")),
        _ => None,
    }
}

/// 判断字符串是否为 PCI ID（如 "10de:2503"）
// Linux 专用；Windows 下仅测试使用，故 allow(dead_code)
#[allow(dead_code)]
fn is_pci_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 9
        && b[4] == b':'
        && s.split(':').all(|h| h.len() == 4 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// 从 lspci -nn 行提取 (设备名, PCI ID)。
/// 例：`01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA106 [GeForce RTX 3060] [10de:2503] (rev a1)`
// Linux 专用；Windows 下仅测试使用，故 allow(dead_code)
#[allow(dead_code)]
fn lspci_identity(line: &str) -> (String, Option<String>) {
    let rest = line.split("]: ").nth(1).unwrap_or(line);
    let mut id = None;
    let mut name = String::new();
    let mut prev_inner: Option<&str> = None;
    let mut pos = 0;
    while let Some(rel) = rest[pos..].find('[') {
        let start = pos + rel;
        let Some(end_rel) = rest[start..].find(']') else { break };
        let inner = &rest[start + 1..start + end_rel];
        if is_pci_id(inner) {
            id = Some(inner.to_lowercase());
            if let Some(p) = prev_inner {
                name = p.to_string();
            }
            break;
        }
        prev_inner = Some(inner);
        pos = start + end_rel + 1;
    }
    if name.is_empty() {
        // 兜底（裸 lspci 无 [10de:xxxx]）：类代码后整段作型号
        name = rest.split('[').next().unwrap_or(rest).trim().to_string();
    }
    (name, id)
}

/// 提取每张显卡的 (型号名, PCI ID) 标识，供限定匹配与 --info 共用
fn gpu_identities_pair(gpus: &[Gpu]) -> Vec<(String, Option<String>)> {
    gpus.iter().map(|g| {
        #[cfg(target_os = "windows")]
        let (name, id) = (g.name.clone(), pci_id_from_pnp(&g.pnp));
        #[cfg(target_os = "linux")]
        let (name, id) = lspci_identity(&g.name);
        (name, id)
    }).collect()
}

/// 显卡限定匹配：白名单非空且无任一显卡（型号名或 PCI ID）命中 -> false（拦截）
fn rule_gpu_match(gpu_rule: &[String], identities: &[(String, Option<String>)]) -> bool {
    if gpu_rule.is_empty() {
        return true; // 未配置限定匹配，不限
    }
    identities.iter().any(|(name, id)| {
        gpu_rule.iter().any(|want| {
            let want = want.trim().to_lowercase();
            !want.is_empty()
                && (name.to_lowercase().contains(&want)
                    || id.as_deref().map(|i| i.to_lowercase().contains(&want)).unwrap_or(false))
        })
    })
}

/// 型号标识（供平台 filter 比对首件与量产是否一致）。
/// 纯文本输出、不写日志；返回 (status, content)，content 每行 `GPU N: <型号> [<vendor:device>]`。
pub fn gpu_identities() -> (bool, String) {
    let (code, gpus) = enum_gpus();
    if code != 0 {
        return (false, format!("枚举显卡失败（rc={code}），无法读取型号"));
    }
    if gpus.is_empty() {
        return (false, "未检测到任何显卡控制器".to_string());
    }
    let smi_rows = if gpus.iter().any(is_nvidia) { nvidia_smi_details() } else { Vec::new() };
    let idents = gpu_identities_pair(&gpus);
    let mut nv_idx = 0usize;
    let lines: Vec<String> = idents
        .iter()
        .enumerate()
        .map(|(i, (name, id))| {
            let g = &gpus[i];
            let mut line = format!(
                "GPU {i}: {}{}",
                name,
                id.as_ref().map(|x| format!(" [{x}]")).unwrap_or_default()
            );
            if is_nvidia(g) {
                if let Some(row) = smi_rows.get(nv_idx) {
                    if let Some(d) = row.get(0).filter(|s| !s.is_empty()) {
                        line.push_str(&format!(" 驱动:{d}"));
                    }
                    if let Some(m) = row.get(1).filter(|s| !s.is_empty()) {
                        line.push_str(&format!(" 显存:{m}"));
                    }
                    if let Some(v) = row.get(2).filter(|s| !s.is_empty()) {
                        line.push_str(&format!(" VBIOS:{v}"));
                    }
                    #[cfg(target_os = "windows")]
                    if !g.driver.is_empty() {
                        line.push_str(&format!(" WMI驱动:{}", g.driver));
                    }
                }
                nv_idx += 1;
            } else {
                #[cfg(target_os = "windows")]
                {
                    if !g.driver.is_empty() {
                        line.push_str(&format!(" 驱动:{}", g.driver));
                    }
                    let vram = vram_mib(g.adapter_ram);
                    if !vram.is_empty() {
                        line.push_str(&format!(" 显存:{vram}"));
                    }
                }
            }
            line
        })
        .collect();
    (true, lines.join("\n"))
}

/// nvidia-smi -L 查询 GPU 列表
fn nvidia_smi_query() -> (i32, String) {
    run_cmd("nvidia-smi", &["-L"])
}

/// Linux: 检查 NVIDIA 内核模块是否加载
#[cfg(target_os = "linux")]
fn nvidia_driver_loaded() -> bool {
    if std::path::Path::new("/proc/driver/nvidia").exists() {
        return true;
    }
    let (code, out) = run_cmd("lsmod", &[]);
    if code == 0 {
        for line in out.lines() {
            if let Some(first) = line.split_whitespace().next() {
                if first == "nvidia" {
                    return true;
                }
            }
        }
    }
    false
}

/// Windows: 从枚举结果找驱动状态异常的 NVIDIA 卡
#[cfg(target_os = "windows")]
fn driver_bad_names(gpus: &[Gpu]) -> Vec<String> {
    gpus.iter()
        .filter(|g| g.status != "OK" && !g.status.is_empty())
        .map(|g| g.name.clone())
        .collect()
}

/// 枚举结果拦截判定：枚举命令失败，或一张显卡都搜不到（结果为空）-> 直接 FAIL，
/// 返回拦截明细；搜到卡返回 None 放行。
fn enum_block_reason(code: i32, gpus: &[Gpu]) -> Option<String> {
    if code != 0 {
        return Some(format!("枚举显卡失败（rc={code}），无法检测显卡"));
    }
    if gpus.is_empty() {
        return Some("未检测到任何显卡控制器，设备枚举异常或显卡完全不识别".to_string());
    }
    None
}

/// 执行显卡识别检测。
///
/// 返回 (status, content)。content 为多行明细，供 e-autotest 界面/日志展示。
/// `gpu_rule` 为限定匹配白名单（型号名或 PCI ID，空 = 不限；非空且无任一显卡命中 -> 拦截）。
/// progress_cb 可选：GUI 用它逐项刷新界面。
pub fn detect(samples: u32, gpu_rule: &[String], progress_cb: Option<ProgressCb>) -> (bool, String) {
    let progress = |stage: Stage, ok: Option<bool>, detail: String| {
        if let Some(cb) = &progress_cb {
            cb(stage, ok, detail);
        }
    };
    let samples = if samples < 1 { 1 } else { samples };
    let mut lines: Vec<String> = Vec::new();

    // 1. 硬件层：枚举显卡控制器
    progress(Stage::Enum, None, "正在枚举显卡设备...".into());
    info!(target: "gpu-test", "硬件枚举: 开始");
    let (code, gpus) = enum_gpus();
    if let Some(detail) = enum_block_reason(code, &gpus) {
        // 搜索不到显卡：直接拦截 FAIL
        let line = format!("FAIL: {detail}");
        progress(Stage::Enum, Some(false), detail.clone());
        error!(target: "gpu-test", "硬件枚举: FAIL {detail}");
        return (false, line);
    }

    lines.push(format!("检测到显卡设备 {} 个:", gpus.len()));
    for g in &gpus {
        #[cfg(target_os = "windows")]
        let line = format!("  {} - 状态:{} 驱动:{}", g.name, g.status, g.driver);
        #[cfg(target_os = "linux")]
        let line = format!("  {}", g.name);
        info!(target: "gpu-test", "硬件枚举: {line}");
        lines.push(line);
        progress(Stage::Enum, Some(true), g.name.clone());
    }

    // 1.1 限定匹配：配置了白名单且无任一显卡命中 -> 拦截（wrong 显卡型号/PCI ID 防呆）
    if !gpu_rule.is_empty() {
        let idents = gpu_identities_pair(&gpus);
        if !rule_gpu_match(gpu_rule, &idents) {
            let detail = format!("限定匹配未命中（允许: {}）", gpu_rule.join(" / "));
            error!(target: "gpu-test", "显卡功能: FAIL {detail}");
            lines.push(format!("FAIL: {detail}"));
            progress(Stage::Enum, Some(false), detail);
            return (false, lines.join("\n"));
        }
        let hit = format!("限定匹配: 命中（允许: {}）", gpu_rule.join(" / "));
        info!(target: "gpu-test", "显卡功能: {hit}");
        lines.push(hit);
    }

    // 所有显卡的驱动状态检查（Windows；兼容核显/AMD/虚拟显示，驱动异常即拦截）
    #[cfg(target_os = "windows")]
    {
        let bad = driver_bad_names(&gpus);
        if !bad.is_empty() {
            let detail = format!("驱动状态异常: {}", bad.join("、"));
            error!(target: "gpu-test", "显卡功能: FAIL {detail}");
            lines.push(format!("FAIL: {detail}"));
            progress(Stage::Enum, Some(false), detail);
            return (false, lines.join("\n"));
        }
    }

    let nvidia_gpus: Vec<&Gpu> = gpus.iter().filter(|g| is_nvidia(g)).collect();
    if nvidia_gpus.is_empty() {
        // 仅核显/非 NVIDIA 独显：不误杀，只记录
        let detail = "未检测到 NVIDIA 独立显卡（仅核显或其他厂商），本项 PASS，不做驱动拦截".to_string();
        info!(target: "gpu-test", "显卡功能: {detail}");
        lines.push(detail.clone());
        progress(Stage::Enum, Some(true), "未检测到 NVIDIA 独立显卡".into());
        return (true, lines.join("\n"));
    }

    // 2. 驱动层：nvidia-smi 可用性
    let nv_count = format!("检测到 NVIDIA 显卡 {} 个，进入驱动与稳定性检查", nvidia_gpus.len());
    info!(target: "gpu-test", "显卡功能: {nv_count}");
    lines.push(nv_count);
    progress(Stage::Driver, None, "正在检查 nvidia-smi 与驱动...".into());
    let (smi_code, smi_out) = nvidia_smi_query();
    if smi_code != 0 || smi_out.trim().is_empty() {
        let mut detail = if smi_code == 0 {
            "nvidia-smi 输出为空（概率性丢失表现）".to_string()
        } else {
            format!("nvidia-smi 不可用（rc={smi_code}）")
        };
        // 补充驱动状态/模块信息
        #[cfg(target_os = "linux")]
        if !nvidia_driver_loaded() {
            detail.push_str("，且 NVIDIA 内核模块未加载（驱动未安装或加载失败）");
        }
        #[cfg(target_os = "windows")]
        {
            let bad = driver_bad_names(gpus.as_slice());
            if !bad.is_empty() {
                detail.push_str(&format!("，驱动状态异常: {}", bad.join("、")));
            }
        }
        let line = format!("FAIL: {detail}");
        error!(target: "gpu-test", "显卡功能: FAIL {detail}");
        lines.push(line.clone());
        progress(Stage::Driver, Some(false), detail);
        return (false, lines.join("\n"));
    }
    let smi_names: Vec<&str> = smi_out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    for n in &smi_names {
        let line = format!("  nvidia-smi: {n}");
        info!(target: "gpu-test", "显卡功能: {line}");
        lines.push(line);
    }
    progress(Stage::Driver, Some(true), format!("nvidia-smi 正常，识别到 {} 个 GPU", smi_names.len()));

    // 3. 功能自检：温度/功耗/时钟/利用率有响应，证明 GPU 功能存活
    progress(Stage::Func, None, "正在功能自检（温度/功耗/时钟）...".into());
    info!(target: "gpu-test", "功能自检: 开始");
    let (fc, fout) = nvidia_smi_stats();
    if fc != 0 || fout.trim().is_empty() {
        let detail = format!("功能自检失败（nvidia-smi rc={fc}，输出为空）");
        error!(target: "gpu-test", "功能自检: FAIL {detail}");
        lines.push(format!("FAIL: {detail}"));
        progress(Stage::Func, Some(false), detail);
        return (false, lines.join("\n"));
    }
    for raw in fout.lines() {
        let l = raw.trim();
        if !l.is_empty() {
            info!(target: "gpu-test", "功能自检: {l}");
            lines.push(format!("  功能自检: {l}"));
        }
    }
    progress(Stage::Func, Some(true), "功能自检正常（温度/功耗/时钟有响应）".into());

    // 4. 稳定性：连续采样 N 次，检测概率性丢失
    progress(Stage::Sample, None, format!("正在稳定性采样（{samples} 次）..."));
    info!(target: "gpu-test", "稳定性采样: 开始，共 {samples} 次");
    let mut failures = 0u32;
    for i in 1..=samples {
        let (rc, out) = nvidia_smi_query();
        if rc != 0 || out.trim().is_empty() {
            failures += 1;
            let extra = if rc == 0 { "，输出为空" } else { "" };
            let line = format!("  采样 {i}/{samples} 失败（nvidia-smi rc={rc}{extra}）");
            warn!(target: "gpu-test", "稳定性采样: {line}");
            lines.push(line);
            progress(Stage::Sample, Some(false), format!("采样 {i}/{samples} 失败"));
        } else {
            let line = format!("  采样 {i}/{samples} 正常");
            info!(target: "gpu-test", "稳定性采样: {line}");
            lines.push(line);
            progress(Stage::Sample, Some(true), format!("采样 {i}/{samples} 正常"));
        }
        if i < samples {
            std::thread::sleep(std::time::Duration::from_millis(SAMPLE_INTERVAL));
        }
    }
    if failures > 0 {
        let line = format!("FAIL: nvidia-smi 连续采样 {failures}/{samples} 次失败，存在概率性丢失");
        error!(target: "gpu-test", "显卡功能: {line}");
        lines.push(line.clone());
        progress(Stage::Sample, Some(false), format!("采样 {failures}/{samples} 次失败，存在概率性丢失"));
        return (false, lines.join("\n"));
    }

    let pass = format!("PASS: 显卡硬件识别正常，驱动已加载，nvidia-smi 采样 {samples} 次全部正常");
    info!(target: "gpu-test", "显卡功能: {pass}");
    lines.push(pass);
    progress(Stage::Sample, Some(true), format!("采样 {samples} 次全部正常"));
    (true, lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_nvidia_by_name_and_pnp() {
        let nv = Gpu { name: "NVIDIA GeForce RTX 3060".into(), pnp: "PCI\\VEN_10DE&DEV_2487".into(), ..Default::default() };
        assert!(is_nvidia(&nv));
        let intel = Gpu { name: "Intel(R) HD Graphics 630".into(), pnp: "PCI\\VEN_8086".into(), ..Default::default() };
        assert!(!is_nvidia(&intel));
    }

    #[test]
    fn test_windows_json_parse_array() {
        let text = r#"[{"Name":"NVIDIA GeForce RTX 3060","Status":"OK","PNPDeviceID":"PCI\\VEN_10DE","DriverVersion":"32.0.16.1062"},{"Name":"OrayIddDriver Device","Status":"OK","PNPDeviceID":"ROOT\\DISPLAY","DriverVersion":"17.50"}]"#;
        let gpus: Vec<Gpu> = serde_json::from_str(&normalize_vc_json(text)).unwrap();
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 3060");
        assert_eq!(gpus[0].pnp, "PCI\\VEN_10DE");
        assert!(is_nvidia(&gpus[0]));
        assert!(!is_nvidia(&gpus[1]));
    }

    #[test]
    fn test_windows_json_parse_single_object() {
        // ConvertTo-Json 单条时不带 []，normalize 后应解析出 1 个
        let text = r#"{"Name":"NVIDIA GeForce RTX 3060","Status":"OK","PNPDeviceID":"PCI\\VEN_10DE","DriverVersion":"32.0"}"#;
        let gpus: Vec<Gpu> = serde_json::from_str(&normalize_vc_json(text)).unwrap();
        assert_eq!(gpus.len(), 1);
        assert!(is_nvidia(&gpus[0]));
    }

    #[test]
    fn test_windows_driver_bad_names() {
        let gpus = vec![
            Gpu { name: "NVIDIA RTX 3060".into(), status: "Error".into(), ..Default::default() },
            Gpu { name: "Intel HD".into(), status: "OK".into(), ..Default::default() },
        ];
        let bad = driver_bad_names(&gpus);
        assert_eq!(bad, vec!["NVIDIA RTX 3060"]);
    }

    #[test]
    fn test_pci_id_from_pnp() {
        assert_eq!(pci_id_from_pnp(r"PCI\VEN_10DE&DEV_2487"), Some("10de:2487".into()));
        assert_eq!(pci_id_from_pnp(r"PCI\VEN_8086&DEV_5912"), Some("8086:5912".into()));
        assert_eq!(pci_id_from_pnp(r"ROOT\DISPLAY"), None);
    }

    #[test]
    fn test_lspci_identity() {
        let line = "01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA106 [GeForce RTX 3060] [10de:2503] (rev a1)";
        let (name, id) = lspci_identity(line);
        assert_eq!(name, "GeForce RTX 3060");
        assert_eq!(id.as_deref(), Some("10de:2503"));
        // 裸 lspci（无 -nn）：整段兜底作型号，无 ID
        let (name2, id2) =
            lspci_identity("01:00.0 VGA compatible controller: NVIDIA Corporation GA106 [GeForce RTX 3060]");
        assert!(id2.is_none());
        assert!(!name2.is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_vram_mib() {
        assert_eq!(vram_mib(Some(12884901888)), "12288 MiB"); // 12GB
        assert_eq!(vram_mib(Some(0)), "");
        assert_eq!(vram_mib(None), "");
    }

    #[test]
    fn test_parse_smi_csv() {
        let out = "610.62, 12288 MiB, 94.04.71.00.c2\n";
        let rows = parse_smi_csv(out);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "610.62");
        assert_eq!(rows[0][1], "12288 MiB");
        assert_eq!(rows[0][2], "94.04.71.00.c2");
        assert!(parse_smi_csv("").is_empty());
    }

    #[test]
    fn test_enum_block_reason() {
        // 枚举命令失败 -> 拦截
        assert!(enum_block_reason(-1, &[]).is_some());
        assert!(enum_block_reason(1, &[]).is_some());
        // 一张卡都搜不到（结果为空）-> 直接拦截 FAIL
        let r = enum_block_reason(0, &[]);
        assert!(r.is_some());
        assert!(r.unwrap().contains("未检测到任何显卡控制器"));
        // 搜到卡 -> 放行
        let ok = Gpu { name: "NVIDIA GeForce RTX 3060".into(), ..Default::default() };
        assert!(enum_block_reason(0, std::slice::from_ref(&ok)).is_none());
    }

    #[test]
    fn test_rule_gpu_match() {
        let idents = vec![("NVIDIA GeForce RTX 3060".to_string(), Some("10de:2503".to_string()))];
        // 空白名单 = 不限
        assert!(rule_gpu_match(&[], &idents));
        // 命中型号名
        assert!(rule_gpu_match(&["RTX 3060".to_string()], &idents));
        // 命中 PCI ID（大小写不敏感）
        assert!(rule_gpu_match(&["10DE:2503".to_string()], &idents));
        // 未命中 -> 拦截
        assert!(!rule_gpu_match(&["RTX 4090".to_string()], &idents));
        // 多个白名单任一个命中即可
        assert!(rule_gpu_match(&["RTX 3090".to_string(), "3060".to_string()], &idents));
        // 白名单空白项被忽略
        assert!(!rule_gpu_match(&[" ".to_string(), "RTX 4090".to_string()], &idents));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_gpu_identities_pair_windows() {
        let gpus = vec![
            Gpu { name: "NVIDIA GeForce RTX 3060".into(), pnp: r"PCI\VEN_10DE&DEV_2487".into(), ..Default::default() },
            Gpu { name: "Intel HD".into(), pnp: "ROOT\\DISPLAY".into(), ..Default::default() },
        ];
        let idents = gpu_identities_pair(&gpus);
        assert_eq!(idents.len(), 2);
        assert_eq!(idents[0].0, "NVIDIA GeForce RTX 3060");
        assert_eq!(idents[0].1.as_deref(), Some("10de:2487"));
        assert_eq!(idents[1].1, None);
    }
}
