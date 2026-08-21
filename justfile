# gpu-test 构建 / 打包 / 验证命令（复刻 etest 跨平台链路）
# 用法:
#   just --list        查看全部命令
#   just setup         首次:一键安装交叉编译工具(Windows)
#   just build-win     构建 Windows 版
#   just build-linux   构建 Linux 版(glibc 2.27 基线)
#   just dist          一键打包:生成 dist/gpu-test-v<版本>.zip,开箱即用
#   just test          单元测试
# 依赖: Windows + VS2022 Build Tools(MSVC v143 x64) + Rust MSVC 工具链; Linux 交叉编译用 cargo-zigbuild + zig

# Windows 下所有 recipe 统一走 PowerShell（不依赖 Git Bash / cmd 脚本）
set windows-shell := ["powershell", "-NoProfile", "-Command"]

# ---- 可变项统一变量（改这里即可，不要散落硬编码）----
bin          := "gpu-test"                       # 程序名（产物名随它变）
profile      := "release"                        # 构建配置 release / debug
target_linux := "x86_64-unknown-linux-gnu.2.27"  # zigbuild 自定义目标（glibc 2.27 基线）
target_short := "x86_64-unknown-linux-gnu"       # 产物目录名（zigbuild 自动去掉 .2.27 后缀）
dist_dir     := "dist"                           # 打包输出目录
docs         := "README.md, README.en.md, LICENSE, gpu-test.toml"  # 打进发布包的文件清单
win_exe      := "target/" + profile + "/" + bin + ".exe"
linux_bin    := "target/" + target_short + "/" + profile + "/" + bin

default:
    @just --list
    @Write-Host ''
    @Write-Host '快速上手:'
    @Write-Host '  just setup        # 首次:安装交叉编译工具(cargo-zigbuild + zig)'
    @Write-Host '  just build-win    # 构建 Windows 版'
    @Write-Host '  just build-linux  # 构建 Linux 版(glibc 2.27 基线)'
    @Write-Host '  just dist         # 一键打包: dist/gpu-test-v<版本>.zip 开箱即用'
    @Write-Host '  just version      # 查看当前版本号(Cargo.toml)'
    @Write-Host '  just test         # 单元测试(需 Windows + VS 环境)'

# 一键安装依赖: zig + cargo-zigbuild（Windows；Linux 用户装 zig 后直接用原生 cargo）
setup:
    @if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { Write-Host '未找到 winget，请手动安装 zig: https://ziglang.org/download/'; exit 1 }
    @if (-not (Get-Command zig -ErrorAction SilentlyContinue)) { winget install -e --id zig.zig --accept-source-agreements --accept-package-agreements } else { Write-Host 'zig 已安装，跳过' }
    @if (-not (Get-Command cargo-zigbuild -ErrorAction SilentlyContinue)) { cargo install cargo-zigbuild --locked } else { Write-Host 'cargo-zigbuild 已安装，跳过' }
    @Write-Host '依赖安装完成。Windows 另需 VS2022 Build Tools(MSVC v143 x64) + Rust MSVC 工具链；Linux 用户装 zig 后可直接 cargo build/test。'

# 读取当前版本号（Cargo.toml 为唯一来源）
version:
    @(Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value

# 打印单个构建产物（build-win / build-linux 复用）
[private]
artifact path:
    @Get-Item {{path}} | Select-Object FullName, Length

# 交叉编译工具检查（build-linux / check 复用）
[private]
ensure-tools:
    @if (-not (Get-Command zig -ErrorAction SilentlyContinue)) { Write-Host '缺少 zig：请先执行 just setup 一键安装'; exit 1 }
    @if (-not (Get-Command cargo-zigbuild -ErrorAction SilentlyContinue)) { Write-Host '缺少 cargo-zigbuild：请先执行 just setup 一键安装'; exit 1 }

# MSVC 环境前缀：vswhere 定位 VS 后借 cmd 设置 vcvars + MSVC 工具链，再执行传入命令
# 默认 GNU 工具链 dlltool 有缺陷，必须走 MSVC（build-win / test / check 统一入口）
[private]
msvc cmd:
    @$vs = & 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe' -all -products * -property installationPath | Select-Object -Last 1; if (-not $vs) { Write-Host '未找到 Visual Studio：build-win/check/test 需 Windows + VS2022 Build Tools(MSVC v143 x64)，请安装后重试（Linux 用户请直接用 cargo build/test）'; exit 1 }; cmd /c "call `"$vs\VC\Auxiliary\Build\vcvars64.bat`" >nul && set `"RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc`" && {{cmd}}"

# Windows 发布版编译（MSVC 工具链）
build-win:
    @just msvc "cargo build --{{profile}}"
    @just artifact {{win_exe}}

# Linux 交叉编译（zig cc，glibc 2.27 基线，Ubuntu 18.04+ / Kylin V10 / UOS V20）
build-linux:
    @just ensure-tools
    cargo zigbuild --{{profile}} --target {{target_linux}}
    @just artifact {{linux_bin}}

# 双目标编译检查（Linux 走 zigbuild）
check:
    @{{ if os() == 'windows' { 'just msvc "cargo check"' } else { 'cargo check' } }}
    @{{ if os() == 'windows' { 'just ensure-tools' } else { 'command -v zig >/dev/null && command -v cargo-zigbuild >/dev/null || { echo "缺少 zig/cargo-zigbuild，请先安装"; exit 1; }' } }}
    @{{ if os() == 'windows' { 'just msvc "cargo zigbuild --target ' + target_linux + '"' } else { 'cargo zigbuild --target ' + target_linux } }}

# 单元测试（MSVC 工具链）
test:
    @{{ if os() == 'windows' { 'just msvc "cargo test"' } else { 'cargo test' } }}

# 一键打包：构建 Windows + Linux 后打成 dist/gpu-test-v<版本>.zip，开箱即用
dist:
    @just build-win
    @just build-linux
    @$v = (& just version | Select-Object -Last 1).Trim(); $d = "{{dist_dir}}/gpu-test-v$v"; if (Test-Path $d) { Remove-Item -Recurse -Force $d }; New-Item -ItemType Directory -Force -Path "$d/windows", "$d/linux" | Out-Null; Copy-Item {{win_exe}} "$d/windows/"; Copy-Item {{linux_bin}} "$d/linux/"; Copy-Item {{docs}} "$d/"; $zip = "{{dist_dir}}/gpu-test-v$v.zip"; if (Test-Path $zip) { Remove-Item -Force $zip }; Compress-Archive -Path $d -DestinationPath $zip; Write-Host ("打包完成: " + (Resolve-Path $zip).Path); Add-Type -AssemblyName System.IO.Compression.FileSystem; $z = [System.IO.Compression.ZipFile]::OpenRead($zip); try { $z.Entries | ForEach-Object { Write-Host ('  ' + $_.FullName) } } finally { $z.Dispose() }

# 清空构建产物（含打包目录）
clean:
    cargo clean
    @if (Test-Path {{dist_dir}}) { Remove-Item -Recurse -Force {{dist_dir}} }
