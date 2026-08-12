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

target_linux := "x86_64-unknown-linux-gnu.2.27"
bin          := "gpu-test"
dist_dir     := "dist"

default:
    @just --list
    @Write-Host ''
    @Write-Host '快速上手:'
    @Write-Host '  just setup        # 首次:安装交叉编译工具(cargo-zigbuild + zig)'
    @Write-Host '  just build-win    # 构建 Windows 版'
    @Write-Host '  just build-linux  # 构建 Linux 版(glibc 2.27 基线)'
    @Write-Host '  just dist         # 一键打包: dist/gpu-test-v<版本>.zip 开箱即用'
    @Write-Host '  just test         # 单元测试(需 Windows + VS 环境)'

# 一键安装依赖: zig + cargo-zigbuild（Windows；Linux 用户装 zig 后直接用原生 cargo）
setup:
    @if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { Write-Host '未找到 winget，请手动安装 zig: https://ziglang.org/download/'; exit 1 }
    winget install -e --id zig.zig --accept-source-agreements --accept-package-agreements
    cargo install cargo-zigbuild --locked
    @Write-Host '依赖安装完成。Windows 另需 VS2022 Build Tools(MSVC v143 x64) + Rust MSVC 工具链；Linux 用户装 zig 后可直接 cargo build/test。'

# MSVC 环境前缀：vswhere 定位 VS 后借 cmd 设置 vcvars + MSVC 工具链，再执行传入命令
# 默认 GNU 工具链 dlltool 有缺陷，必须走 MSVC（build-win / test / check 统一入口）
[private]
msvc cmd:
    @$vs = & 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe' -all -products * -property installationPath | Select-Object -Last 1; if (-not $vs) { Write-Host '未找到 Visual Studio：build-win/check/test 需 Windows + VS2022 Build Tools(MSVC v143 x64)，请安装后重试（Linux 用户请直接用 cargo build/test）'; exit 1 }; cmd /c "call `"$vs\VC\Auxiliary\Build\vcvars64.bat`" >nul && set `"RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc`" && {{cmd}}"

# Windows 发布版编译（MSVC 工具链）
build-win:
    @just msvc "cargo build --release"
    @Get-Item target\release\gpu-test.exe | Select-Object FullName, Length

# Linux 交叉编译（zig cc，glibc 2.27 基线，Ubuntu 18.04+ / Kylin V10 / UOS V20）
build-linux:
    @if (-not (Get-Command zig -ErrorAction SilentlyContinue)) { Write-Host '缺少 zig：请先执行 just setup 一键安装'; exit 1 }
    @if (-not (Get-Command cargo-zigbuild -ErrorAction SilentlyContinue)) { Write-Host '缺少 cargo-zigbuild：请先执行 just setup 一键安装'; exit 1 }
    cargo zigbuild --release --target {{target_linux}}
    @Get-Item target\x86_64-unknown-linux-gnu\release\gpu-test | Select-Object FullName, Length

# 双目标编译检查（Linux 走 zigbuild）
check:
    @just msvc "cargo check"
    @just msvc "cargo zigbuild --target {{target_linux}}"

# 单元测试（MSVC 工具链）
test:
    @just msvc "cargo test"

# 一键打包：构建 Windows + Linux 后打成 dist/gpu-test-v<版本>.zip，开箱即用
dist:
    @just build-win
    @just build-linux
    @$v = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value; $d = "{{dist_dir}}/gpu-test-v$v"; if (Test-Path $d) { Remove-Item -Recurse -Force $d }; New-Item -ItemType Directory -Force -Path "$d/windows", "$d/linux" | Out-Null; Copy-Item target\release\gpu-test.exe "$d/windows/"; Copy-Item target\x86_64-unknown-linux-gnu\release\gpu-test "$d/linux/"; Copy-Item README.md, README.en.md, LICENSE "$d/"; $zip = "{{dist_dir}}/gpu-test-v$v.zip"; if (Test-Path $zip) { Remove-Item -Force $zip }; Compress-Archive -Path $d -DestinationPath $zip; Write-Host ("打包完成: " + (Resolve-Path $zip).Path)

# 清空构建产物（含打包目录）
clean:
    cargo clean
    @if (Test-Path {{dist_dir}}) { Remove-Item -Recurse -Force {{dist_dir}} }
