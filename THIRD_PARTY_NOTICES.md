# 第三方代码与许可说明

本项目版本为 v0.8.0。以下说明用于保留代码来源和主要依赖的许可信息，不替代各组件的许可证原文。

## Muguet

- 来源：[c22dev/muguet](https://github.com/c22dev/muguet)。
- 作者：Constantin Clerc（c22dev）及上游贡献者，原文件的作者及版权声明继续保留。
- 使用基线：`1a0300d3cfc5f87ce2fa1ccb365c013e5238e322`，记录见 `upstream/baseline.json`。
- 许可：**GPL-3.0-or-later**，由该基线的 `Cargo.toml` 明确指定；GPLv3 全文保留于根目录 [LICENSE](LICENSE) 和 [game-runtime/LICENSE](game-runtime/LICENSE)。
- 涉及范围：`game-runtime/` 中移植的宿主接口、文件系统、图形、音频、输入、补丁及相关 WGSL 辅助代码。
- 修改说明（2026-10-18，v0.8.0）：替换 Apple 平台入口与接口，增加 OHOS 静态库及签名 AOT 加载、ArkTS / C++ 桥接、Vulkan 窗口、OHAudio、输入及生命周期适配、图形优化和空间超分；为源码分发移除对私有游戏资源的编译期依赖。
- 本地资源导入（2026-10-18）：增加公共下载目录、文件管理 ZIP 入口、ZIP64 流式解压与校验，以及资源替换和启动互斥。此功能不会从资源包加载或执行 AOT、Wasm 或 HAP。

分发这些衍生代码时必须保留原许可与署名；分发对应二进制时，须按 GPL 提供对应源码与构建脚本。仅链接上游仓库不能替代提供本项目修改版源码。

## 图形与运行时依赖

| 组件 | 当前主要版本 | 许可 | 来源 / 本地保留位置 |
| --- | --- | --- | --- |
| Wasmtime / Cranelift | 49.0.0 / 0.136.0 | Apache-2.0 WITH LLVM-exception | [bytecodealliance/wasmtime](https://github.com/bytecodealliance/wasmtime)；原文见 [Wasmtime-LICENSE](licenses/Wasmtime-LICENSE) |
| wgpu、wgpu-core、wgpu-hal、Naga | 30.0.1 | MIT OR Apache-2.0 | [gfx-rs/wgpu](https://github.com/gfx-rs/wgpu)；修改后的 core / hal 在 `game-runtime/vendor/` |
| ash | 0.38.0 | MIT OR Apache-2.0 | [ash-rs/ash](https://github.com/ash-rs/ash) |
| audiopus_sys / Opus | 0.2.2 / 随该 crate 的原生版本 | 包装层 ISC；Opus 保留其 BSD 式版权与免责声明 | [lakelezz/audiopus_sys](https://github.com/lakelezz/audiopus_sys)；见 [包装层许可](licenses/audiopus_sys-LICENSE.md)、[Opus-COPYING](licenses/Opus-COPYING) 及[补充说明](licenses/Opus-LICENSE_PLEASE_READ.txt) |
| bcdec_rs | 0.2.0 | MIT | [ScanMountGoat/image_dds](https://github.com/ScanMountGoat/image_dds/tree/main/bcdec_rs) |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | [rust-windowing/raw-window-handle](https://github.com/rust-windowing/raw-window-handle) |

`anyhow`、`libc`、`serde_json`、`sha2`、`regex`、`rustc-hash`、`pollster` 等依赖也保留其原有许可。传递依赖中的 `unicode-ident` 还涉及 Unicode-3.0，`foldhash` 使用 Zlib，`libloading` 使用 ISC，需随实际分发内容保留相应声明。具体解析版本以各子工程的 `Cargo.lock` 为准；以上是主要依赖说明，不是完整的传递依赖清单。

`game-runtime/vendor/wgpu-core/` 与 `game-runtime/vendor/wgpu-hal/` 均保留原 `LICENSE.MIT` 和 `LICENSE.APACHE`。修改版继续保留这些许可证；不能用根目录 GPL 文本替换第三方许可证。发布静态链接的二进制时，还需附上实际纳入组件的版权、许可及适用的 NOTICE / 例外文本，包括 Wasmtime 的 LLVM exception 和 Opus 随附声明。

## SDK、系统库与游戏文件

HarmonyOS / OpenHarmony、HMS native（含 XEngine / FrameGeneration）头文件与库由用户通过官方 SDK 获取，遵守各自许可及开发者条款；本仓库不随源码打包这些 SDK。

ZIP 导入通过 HarmonyOS 系统提供的 `libz.so` 使用 zlib 的 Deflate 和 CRC 接口，不另行打包 zlib 动态库。zlib 由 Jean-loup Gailly 与 Mark Adler 编写，采用 zlib 许可；SDK 头文件中的许可声明保留于 [zlib-LICENSE](licenses/zlib-LICENSE)。

游戏程序、数据、游戏着色器及其 AOT 派生产物不属于本项目的开源授权范围。源码仓库不包含这些文件；完整 HAP 内的游戏 AOT 不适用本仓库源码的开源许可。本地 `resources/`、`game-runtime/artifacts/`、`releases/` 和测试捕获记录均不属于公开源码。

应用图标将 GTA V 的绿色 V 与鸿蒙圆环元素融合，由图像生成工具制作；相关标志的版权及商标权仍归原权利人。图标来源与制作说明见 [ICON_SOURCES.md](docs/assets/ICON_SOURCES.md)，不将第三方标志重新声明为 GPL 授权。
