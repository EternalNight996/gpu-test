# gpu-test — GPU 功能性测试插件（Rust）

> 给用户/产线操作员看的说明见 [README-用户版.md](README-用户版.md)。
> 给客户看的方案说明（含改善前后对比与效益）见 [README-客户版.md](README-客户版.md)。

面向产线测试工位的 GPU 功能性检测插件，兼容所有显卡（核显 / NVIDIA / 其他独显），
当前先完成 NVIDIA 深度检测。检测显卡是否被系统识别、驱动是否正常加载、
`nvidia-smi` 是否稳定（连续采样防"概率性丢失"），对不良显卡自动拦截；
支持 GUI 弹窗人工确认与 `--no-gui` 自动化两种模式，可接入 e-autotest 测试平台
（`R<{json}>R` 标准输出 + 退出码 + e-log 文件/stdout 双日志）。

## 参与开发

- 构建：`just build-win`（Windows）/ `just build-linux`（Linux 交叉编译，glibc 2.27 基线）
- 测试：`just test`；新增检测逻辑请同步补充 `src/detect.rs` 中的单元测试
- 架构：`src/detect.rs` 检测核心（Windows WMI / Linux lspci 枚举 + nvidia-smi 功能检测）、
  `src/gui.rs` egui 界面、`src/logger.rs` e-log 日志、`src/main.rs` CLI/GUI 入口
- 欢迎提交 PR：修复、新显卡厂商支持、新检测项等

- 语言：Rust（egui 弹窗 + `--no-gui` CLI 模式）
- 平台：Windows 10/11、Linux（Ubuntu 18.04+ / 银河麒麟 V10 / 统信 UOS V20，x86_64）
- 接入方式：e-autotest 插件标准（独立可执行程序 + `R<json>R` 输出 + 退出码）

---

## 一、检测逻辑

| # | 检测项 | 判定 | 对应不良现象 |
|---|---|---|---|
| 1 | 枚举显卡控制器（Windows `Win32_VideoController` / Linux `lspci -nn` VGA·3D·Display） | 检测不到任何显卡 → **FAIL**（枚举异常） | 显卡完全不识别 |
| 2 | NVIDIA 显卡存在时，检查 `nvidia-smi` 可用性 | `nvidia-smi` 不可用或输出为空 → **FAIL**（Windows 附驱动状态异常明细，Linux 附内核模块未加载明细） | 驱动未装/加载失败 |
| 3 | 功能自检：利用率/功耗/温度/时钟是否有响应 | 无响应/查询失败 → **FAIL**（GPU 功能不存活） | GPU 功能异常、传感器无读数 |
| 4 | `nvidia-smi -L` 连续采样 N 次（默认 3） | 任一次失败 → **FAIL**（概率性丢失） | nvidia-smi 概率丢失、概率性不识别 |
| — | 仅核显 / 无 NVIDIA 独显 | **PASS**，content 记录"仅核显"，不做驱动拦截（不误杀核显机型） | — |

> 判定规则：**有 NVIDIA 独显时**，驱动/smi 任一异常即拦截并报明细；
> **无独显（核显 only）** 不算 FAIL，只记录。采样次数可用 `--samples` 调整。

## 二、构建与自测

```bash
just build-win     # Windows 发布版 → target/release/gpu-test.exe
just build-linux   # Linux 交叉编译（glibc 2.27 基线）→ target/x86_64-unknown-linux-gnu/release/gpu-test
just test          # 单元测试（检测核心，无需真机），4 个用例
```

Windows 构建需 Visual Studio（MSVC 工具链，GNU dlltool 有缺陷）；Linux 交叉编译需
cargo-zigbuild + zig。构建命令统一走 `justfile`，无独立脚本。

## 三、部署步骤（接入 e-autotest 平台）

### 1. 放置插件文件

```bash
# 假设 e-autotest 安装在 /opt/e-autotest（Windows 为安装根目录）
mkdir -p /opt/e-autotest/plugins/gpu-test
cp gpu-test /opt/e-autotest/plugins/gpu-test/   # Linux 版（无后缀）
chmod +x /opt/e-autotest/plugins/gpu-test/gpu-test
# Windows 版：拷贝 gpu-test.exe 到 plugins\gpu-test\
```

### 2. 平台注册 APP（extend_app 配置）

在 e-autotest 配置界面"APP 配置"新增一条，或在数据库 `extend_app` 表插入：

| 字段 | 值 | 说明 |
|---|---|---|
| `tag` | `GPU_TEST` | APP 标识（流程中引用） |
| `app_type` | 读 APP / 读写在读 APP（按工站用途选） | — |
| `description` | `显卡功能性测试（镭晨）` | 界面显示名 |
| `fileinfo.fname` | `gpu-test.exe`（Windows）/ `gpu-test`（Linux） | 程序名 |
| `fileinfo.cwd` | `plugins/gpu-test` | 工作目录（相对 e-autotest 根，正斜杠） |
| `fileinfo.exe_type` | `WindowsExe` / `LinuxExe` | 原生可执行文件 |
| `fileinfo.architecture` | `X86_64` | 兆芯/海光均为 x86_64 |
| `fileinfo.platform` | `Windows` / `Linux` | 按工站系统选 |
| `args` | `--sn --station --mode --samples 3 --no-gui` | 自动化跑批；想去掉弹窗确认用 `--no-gui`，人工确认工站可省略 |
| `filter` | 留空（插件自行判定） | 插件在 content 中自带 PASS/FAIL 与明细 |
| `timeout` | 30（秒） | 采样 3 次 × smi 约 1s + 余量（GUI 人工确认模式需放宽） |
| `is_check` | true | 需要解析 `R<...>R` 结果 |

> `fileinfo` JSON 参考（Linux）：
> ```json
> {
>   "fname": "gpu-test",
>   "cwd": "plugins/gpu-test",
>   "exe_type": "LinuxExe",
>   "architecture": "X86_64",
>   "platform": "Linux",
>   "is_lib": false,
>   "is_64": true,
>   "libs": []
> }
> ```

### 3. 流程挂载（extend_task）

在测试流程（任务配置）中将该 APP 挂到显卡检测工站，`on_fail` 动作设为拦截
（重测 / 上报 MES NG），实现不良显卡拦截。

### 4. 验证

```bash
# Linux 真机（自动化模式）
/opt/e-autotest/plugins/gpu-test/gpu-test --no-gui --sn TEST001 --station FCT1 --mode AUTO
# 预期输出一行 R<json>R，content 含显卡明细；退出码 0 = 通过，非 0 = 失败
echo $?
```

## 四、命令行参数

| 参数 | 说明 | 默认 |
|---|---|---|
| `--sn` | 序列号（平台自动传入，手动可省略，仅写日志） | 空 |
| `--station` | 工站（平台自动传入，手动可省略，仅写日志） | 空 |
| `--mode` | 模式（平台自动传入，手动可省略，仅写日志） | 空 |
| `--samples` | nvidia-smi 稳定性采样次数 | 3 |
| `--info` | 只输出一键同步标识（`GPU: 型号 [10de:xxxx] 显存`），供平台 filter 比对 | 关 |
| `--no-gui` | 不弹窗，命令行直接输出（自动化/调试） | 关 |
| `--auto` | GUI 检测完成后倒计时自动关闭 | 关（需人工点确认） |
| `--close SECS` | 自动关闭倒计时秒数 | 5 |
| `--res PATH` | 额外把 `R<json>R` 写入文件（平台 res_url 方式） | 空 |

## 四-1、首件基准与量产比对（型号一致性）

功能性检测（驱动/nvidia-smi/稳定性采样）走 GUI 或正常模式拦截；
**型号标识单独走 `--info` 接口**，供平台按"首件基准 vs 量产逐台"比对：

1. **首件采集**：首件运行 `gpu-test --info`，返回一键同步标识：
   ```
   R<{"content":"GPU 0: NVIDIA GeForce RTX 3060 [10de:2487] 驱动:610.62 显存:12288 MiB VBIOS:94.04.71.00.c2 WMI驱动:32.0.16.1062\nGPU 1: OrayIddDriver Device 驱动:17.50.19.949","status":true,"opts":{...}}>R
   ```
   `--info` 保留字段：**型号 + PCI 设备 ID + 驱动版本（nvidia-smi/WMI）+ 显存 + VBIOS**；
   PCI 总线位置、GPU UUID 量产中会变化，不参与一键同步比对。
2. **一键同步**：在 e-autotest APP 配置中把该 APP 的"校验筛选"填为基准值并保存
   （`*10de:2487*` 按 PCI ID 精确比对，或 `*RTX 3060*` 按型号名）：
   ```sql
   UPDATE extend_app SET filter='*10de:2487*' WHERE tag='GPU_TEST_INFO';
   ```
3. **量产比对**：每台执行 `--info`，平台 filter 自动校验，返回型号与基准不一致
   即判 FAIL/NG 拦截；一致则 PASS。

建议注册两个独立 APP：`GPU_TEST`（功能检测，GUI/`--no-gui`）与
`GPU_TEST_INFO`（型号标识，`args: --info`），职责分离、互不干扰。

## 五、输出格式（e-autotest 插件标准）

- 单行 `R<{json}>R`，主程序取最后一个 `R<...>R` 标签解析；
- JSON：`{"content": "...", "status": true|false, "opts": {...}}`；
- 退出码：`0` = PASS，非 `0` = FAIL（与 `status` 一致）；
- UTF-8 编码，输出后强制 flush；
- stdout 同步输出**详细运行日志**（e-log 格式，含时间戳），末尾再输出裸 `R<...>R` 供平台解析；
- 失败时 `content` 含多行明细（显卡列表 / smi 状态 / 采样失败次数），便于界面与日志定位。

## 六、注意事项

- Windows 单版本 `gpu-test.exe`：release 为 GUI 子系统（双击运行只出图形界面、不弹命令行窗口），启动时通过 `e-log` 的 `reattach_windows_terminal()` 挂父控制台（与 heg-os-active2 同款）——从 cmd/PowerShell 运行也能看到 stdout 日志与 `R<...>R` 结果；cmd 对 GUI 程序不等待，如需等待可加 `start /wait` 或用平台管道调用。
- 日志框架统一用 `e-log`（与 e-autotest/etest 生态一致），**同时输出到文件 `logs/gpu-test.log` 与 stdout**（均为 e-log 格式，便于后续按格式筛选数据）；**显卡功能性测试全流程分步记录**（运行开始 → 硬件枚举逐卡 → 驱动/nvidia-smi → 功能自检 → 稳定性每次采样 → 最终判定），每条带时间戳，文件末尾附一行 `R<...>R` 平台格式输出，stdout 末尾另附裸 `R<...>R` 供平台解析。
- Linux 真机需 `lspci`（pciutils，Ubuntu 默认已装）与 `nvidia-smi`（随 NVIDIA 驱动安装）。
- 无独显机型（核显 only）默认 PASS，仅记录；若产线要求"必须独显"，可自行调整 `detect.rs` 判定。
- 采样次数越多拦截越严（防概率性丢失），但耗时线性增加：`--samples 3` 约 3~5s。
- GUI 模式在无显示器/远程会话下自动关闭会被系统重绘节流拖慢，自动化场景请用 `--no-gui`。
- 不良显卡明细（17 块盈通，2026/1–8）见 `docs/images.png`，作为检测项设计依据。
