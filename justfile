# gpu-test 构建 / 打包 / 验证命令（复刻 etest 跨平台链路）
# 用法: just --list 查看全部命令; 首次: cargo install just
# 依赖: cargo zigbuild、cargo-deb、zig（Windows MSVC 需 Visual Studio）

# Windows 下所有 recipe 统一走 PowerShell（不依赖 Git Bash / cmd 脚本）
set windows-shell := ["powershell", "-NoProfile", "-Command"]

target_linux := "x86_64-unknown-linux-gnu.2.27"
target_short  := "x86_64-unknown-linux-gnu"
bin           := "gpu-test"

default:
    @just --list

# MSVC 环境前缀：vswhere 定位 VS 后借 cmd 设置 vcvars + MSVC 工具链，再执行传入命令
# 默认 GNU 工具链 dlltool 有缺陷，必须走 MSVC（build-win / test / check 统一入口）
[private]
msvc cmd:
    @$vs = & 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe' -all -products * -property installationPath | Select-Object -Last 1; if (-not $vs) { Write-Host 'Visual Studio not found'; exit 1 }; cmd /c "call `"$vs\VC\Auxiliary\Build\vcvars64.bat`" >nul && set `"RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc`" && {{cmd}}"

# Windows 发布版编译（MSVC 工具链）
build-win:
    @just msvc "cargo build --release"

# Linux 交叉编译（zig cc，glibc 2.27 基线，Ubuntu 18.04+ / Kylin V10 / UOS V20）
build-linux:
    cargo zigbuild --release --target {{target_linux}}

# 双目标编译检查（Linux 走 zigbuild）
check:
    @just msvc "cargo check"
    @just msvc "cargo zigbuild --target {{target_linux}}"

# 单元测试（MSVC 工具链）
test:
    @just msvc "cargo test"

# 清空构建产物
clean:
    cargo clean

# 说明：本插件以"插件形式"交付（放入平台 plugins/gpu-rayid/，平台通过 extend_app 配置调用），
# 不打 deb（区别于 etest 主程序）。如需 deb 请参考 etest 的 justfile deb 目标。
