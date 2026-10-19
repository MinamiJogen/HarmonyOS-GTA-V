# 编译与资源部署（v0.8.0）

更新日期：2026-10-19。

以下命令在项目根目录执行。构建完整游戏运行包需要用户自己的兼容游戏文件；本仓库不提供这些文件，也不提供下载来源。

## 下载与安装

[v0.8.0 Release](https://github.com/MinamiJogen/HarmonyOS-GTA-V/releases/tag/v0.8.0) 提供 `HarmonyOS-GTA-V-v0.8.0.hap`，目标为 HarmonyOS API 26 / ARM64，应用名称为 `GTA V`，包名为 `com.shuyuan.harmonyosgtav`。该 HAP 包含游戏运行时和 ARM64 AOT，未签名；需先用自己的 DevEco 开发者配置完成签名，再安装到其 profile 允许的设备。

游戏数据和匹配的着色器仍需通过应用首页导入。源码仓库提供运行时、构建脚本和资源导入界面，不包含本机生成的游戏 AOT 或私有游戏文件。自行构建完整 HAP 时，按下文使用自己的兼容文件生成 AOT。

Release 提供完整对应源码 `HarmonyOS-GTA-V-v0.8.0-source.tar.gz` 和 `SHA256SUMS`，下载后可在同一目录运行 `shasum -a 256 -c SHA256SUMS` 校验。自行签名会改变 HAP 内容，校验应在重新签名前进行。`v0.8.0` 标签和 GitHub 的 `Source code` 下载指向该版本对应的公开源码。

## 重新构建并签名公开应用壳

仅构建应用壳需要 macOS、DevEco Studio / API 26 与 HMS native SDK，以及 Python 3；不需要游戏文件或 Rust 工具链。先执行：

```sh
bash tools/build_public_shell.sh
```

工具在全新的 ASCII 临时目录构建并检查无游戏壳包，不读取游戏资源或签名材料，不覆盖完整游戏包。未签名公开产物位于 `dist/public-v0.8.0/HarmonyOS-GTA-V-v0.8.0-shell-arm64-unsigned.hap`。

若要安装到自己的设备，在日志中找到 `Build stage: /private/tmp/harmony-gta5-public-shell.XXXXXXXX/harmony`，用 DevEco 打开这个 `harmony/` 目录并配置自己的自动签名。将下方路径换成日志中的实际临时目录根路径（不含末尾的 `/harmony`），再回到源码根目录执行：

```sh
task_public_shell_stage="/private/tmp/harmony-gta5-public-shell.XXXXXXXX"
HARMONY_BUILD_STAGE="$task_public_shell_stage" \
HARMONY_SHELL_ONLY=1 \
HARMONY_COPY_BUILD_OUTPUT=0 \
HARMONY_WITH_VVL=0 \
HARMONY_BUILD_MODE=release \
./harmony/build_signed_local.sh
```

自己的签名壳包位于 `$task_public_shell_stage/harmony/entry/build/default/outputs/default/entry-default-signed.hap`，通过 DevEco / HDC 安装到 profile 允许的设备。签名后仍是无游戏壳包；完整游戏的构建步骤如下。

## 使用构建工具

安装下方列出的环境后，推荐通过统一入口操作：

```sh
# 只检查环境；不会安装工具、编译、提取资源或连接设备
bash tools/build_game.sh --check

# 从用户自备的兼容 ZIP 构建完整未签名 HAP
bash tools/build_game.sh --zip /absolute/path/your-game.zip

# 或者：使用已准备好的 game.wasm，不重复提取 ZIP
bash tools/build_game.sh --wasm /absolute/path/game.wasm

# 只编译 AOT；不构建应用，也不要求完整 HarmonyOS SDK
bash tools/build_game.sh --wasm /absolute/path/game.wasm --aot-only

# 已配置好 DevEco 镜像中的签名后，生成自己的签名包
bash tools/build_game.sh --wasm /absolute/path/game.wasm --signed
```

也可双击根目录 `build.command`，按中文提示输入游戏 ZIP / Wasm 路径并选择模式。工具复用下文各阶段脚本；不会下载游戏、执行 ZIP 内脚本、自动安装 HAP 或改动设备数据。路径含空格时，命令行参数请用引号包住。

| 参数 | 作用 |
| --- | --- |
| `--zip PATH` | 先校验并提取用户自备 ZIP 到本地 `resources/game/`，再选择唯一的 `b/*/game.wasm` |
| `--wasm PATH` | 直接编译给定的兼容 WebAssembly 游戏程序；外置数据仍需另行准备 |
| `--check` | 只检查环境，可与 `--aot-only` 组合 |
| `--aot-only` | 只生成 `game-runtime/artifacts/game-ohos.cwasm` 及本地补丁 Wasm |
| `--signed` | 用已在 DevEco 镜像中配置的本地签名构建 HAP |
| `--offline` | 禁止 Cargo 下载依赖；需要提前缓存 AOT 工具与运行时依赖 |
| `--help` | 显示完整帮助 |

`--zip` 与 `--wasm` 二选一；默认按 release 模式生成未签名包。首次运行可能需要下载和编译 Rust 依赖，时间取决于网络和电脑；已有工具的历史记录中，约 60 MiB 的 `game.wasm` 转为约 201 MiB AOT 用时约 47 秒，这不包含整个应用构建。游戏构建版本必须与补丁、宿主接口和着色器匹配。

## 环境准备

- macOS、DevEco Studio、HarmonyOS API 26 SDK，包含 HMS native 的 XEngine / FrameGeneration 头文件及链接库。
- Rust / rustup、Rust `1.99.1`、`aarch64-unknown-linux-ohos` 目标。
- Python 3、`rg`、SDK 自带的 Clang / CMake，以及系统 `rsync`。
- 能够为目标设备签名的开发者配置；仓库不包含证书、私钥或设备 profile。

使用系统已有的 Rust 安装时，先设置：

```sh
export MG_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export MG_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
rustup toolchain install 1.99.1
rustup target add --toolchain 1.99.1 aarch64-unknown-linux-ohos
```

统一构建工具会优先复用本地 `runtime-probe/.toolchain/`，缺失时选择用户已有 Rust 安装。手工调用底层脚本时，可用上述两个变量显式指定 Rust 路径；本地工具链目录不会随源码分发。

默认 DevEco 路径为 `/Applications/DevEco-Studio.app/Contents`。其他路径可设置 `DEVECO_CONTENTS`，并将 `OHOS_SDK_NATIVE` 设置为该安装下的 `sdk/default/openharmony/native`。

## 准备游戏文件

所需资源结构如下，`<build>` 下的程序与着色器必须属于同一个兼容版本：

```text
resources/game/
├── data/
│   └── manifest.json      # 以及 manifest 引用的全部数据
└── b/<build>/
    ├── game.wasm
    └── shaders/
        └── index.json    # 以及全部匹配的着色器
```

如已有合法取得的兼容 ZIP，可使用以下工具校验并提取到本地；不会执行 ZIP 内脚本或修改原始 ZIP：

```sh
python3 tools/prepare_game_resources.py \
  /absolute/path/your-game.zip resources/game evidence/resources/prepare-report.json
```

`resources/` 与提取记录均被忽略，不应提交或发布。

## 编译完整运行包

以下是统一工具内部使用的分阶段命令，适用于需要单独排查某一步的开发者。将路径替换为自己的 `game.wasm`：

```sh
bash game-runtime/scripts/compile-aot.sh /absolute/path/game.wasm
bash game-runtime/scripts/build-ohos.sh
bash game-runtime/scripts/embed-game.sh
HARMONY_SHELL_ONLY=0 HARMONY_ENABLE_SIGNING=false HARMONY_BUILD_MODE=release ./harmony/build_unsigned.sh
```

AOT 编译器与运行时固定使用 Wasmtime `49.0.0` 和各自的 `Cargo.lock`。首次构建会下载依赖；已缓存依赖后可用 `MG_AOT_OFFLINE=1` 禁止 AOT 工具联网。CMake 会自动链接生成的运行时静态库和 `game-embedded.o`。

构建脚本会在 `/private/tmp/harmony-runtime-probe-$UID/harmony` 创建 ASCII 路径镜像，解决 Hvigor 对中文路径的限制，再将输出复制回项目。未签名 HAP 位于：

```text
harmony/entry/build/default/outputs/default/entry-default-unsigned.hap
```

该完整无签名包已链接游戏运行时并内嵌游戏 AOT，游戏数据和匹配着色器仍通过资源导入提供。`tools/build_public_shell.sh` 生成的公开壳包会排除 AOT，属于另一种产物。

可以用匹配构建的未剥离原生库校验完整无签名包。将路径替换为实际 HAP、同一构建的 `obj/arm64-v8a/libharmony_probe.so` 和报告输出位置：

```sh
python3 harmony/verify_game_hap.py --unsigned \
  --hap /absolute/path/entry-default-unsigned.hap \
  --elf /absolute/path/obj/arm64-v8a/libharmony_probe.so \
  --report /absolute/path/game-aot-verification.json
```

校验包括包名、图标、游戏运行时链接、包内 AOT 字节与本地编译产物的 SHA-256 一致性、AOT 所在可执行 LOAD 段，以及官方工具确认的无签名状态。

用 DevEco 打开上述临时镜像，配置自己的自动签名，再执行：

```sh
HARMONY_BUILD_MODE=release ./harmony/build_signed_local.sh
```

若设置了 `HARMONY_BUILD_STAGE`，请打开该目录下的 `harmony/`。签名包输出到同一 `outputs/default/` 目录。`release` 是代码构建模式，不代表已取得面向所有设备的分发签名；本地设备签名只适用于其 profile 允许的设备。

## 安装与部署资源

通过 DevEco / HDC 安装自己的已签名 HAP。应用包名为 `com.shuyuan.harmonyosgtav`，入口为 `EntryAbility`。安装后，正常导入游戏资源无需 HDC 或调试权限。

### 通过文件管理导入 ZIP

以下界面适用于当前源码构建的安装包和 v0.8.0 Release 完整 HAP。单独构建的公开壳包可导入资源，但仍需包含 AOT 和运行时的完整 HAP 才能启动游戏。

1. 启动应用，首页会先检查已导入的资源。点击“打开下载目录”，通过系统 `DOWNLOAD` 模式创建并打开本应用的下载专属目录。它可能显示为 `Download/GTA V`；不要把示例当作固定绝对路径，请以系统返回和首页显示的目录为准。
2. 在文件管理中，把自己准备的游戏资源 ZIP 复制到该目录。回到应用刷新 ZIP 列表，点选文件并导入。
3. 文件位于其他位置时，可直接点击“选择游戏 ZIP”，使用系统文件选择器选择任意可访问位置的 ZIP，无需先复制到下载目录。
4. 等待完整校验和导入成功，确认首页显示资源已就绪后开始游戏。

支持 ZIP / ZIP64 大文件。首页显示解压、校验等阶段和导入进度；需要中止时点击“取消导入”，等待临时资源清理完成，已有资源、存档和设置保持不变。

也可在系统文件管理对 ZIP 选择“打开方式 → GTA V”，通过 ZIP `FileOpen` 入口，由系统授权应用只读访问所选文件。如果设备未显示此选项，使用应用内“选择游戏 ZIP”即可；系统“打开方式”面板及授权行为可能因设备而异。

当前只接受与安装包匹配的 `8b0b5899ed` 游戏资源构建。约 15.7 GB 的本地备份 ZIP 需要另外约 15.6 GB 可用空间用于解压；替换时旧资源也会保留到新资源校验完成，原 ZIP 不会自动删除。

应用内导入支持以下两种 ZIP 布局，其中 `<build>` 的游戏着色器必须与当前完整安装包中嵌入的游戏 AOT 匹配：

```text
# 带 game/ 顶层目录的资源包
game/
├── data/manifest.json          # 以及该清单引用的全部数据
└── b/<build>/shaders/index.json # 以及该索引引用的全部游戏着色器

# 或直接从 ZIP 根目录开始
data/manifest.json
b/<build>/shaders/index.json
```

导入功能只处理游戏数据与游戏着色器，不导入或执行压缩包中的 AOT、HAP 或 Wasm，也不会替换应用内已经签名的程序。导入 ZIP 可以不含 `game.wasm`；这与编译工具的 `--zip` 不同，后者需要 `b/<build>/game.wasm` 来生成 AOT。

完整 HAP 构建完成后，可生成仅含运行资源的配套 ZIP：

```sh
python3 tools/package_game_import.py
# 可选：指定自己的已校验资源目录和输出路径
python3 tools/package_game_import.py --game-dir /absolute/path/game --output /absolute/path/runtime-resources.zip
```

工具要求资源目录含原有 `asset-sha256.txt` 校验表，逐个校验后只压缩 `data/` 和 `b/8b0b5899ed/shaders/`。ZIP 中不保留原始 `game.wasm`、重复 AOT、安装包或编译源码；校验表及打包报告留在 ZIP 外。本地源文件不会删除。默认产物与 SHA-256 文件位于 `dist/private-import-v0.8.0/`。此 ZIP 配合内置匹配 AOT 的完整 HAP 使用，未签名 HAP 仍需先用自己的开发者配置签名后安装；完整 HAP 和资源 ZIP 均包含游戏文件或派生产物，仅供本地使用，不作为公开 Release 附件。

新资源先在临时目录解压，校验每个导入成员的 CRC 和长度、manifest 的文件引用与大小、着色器索引引用及常量 JSON，再切换应用私有目录中的旧资源；导入失败时保留原有资源，不改动 `filesDir/saves/` 或 `filesDir/config.json`。这些校验不等于全部着色器编译或真机游戏验证。游戏启动后不支持在同一进程中导入并替换资源；需要更换时，先彻底退出并重新启动应用，回到首页后再操作。

**无游戏应用壳只导入数据仍不能运行游戏。** 它没有嵌入游戏 AOT；需要使用自己合法取得的兼容游戏程序在本地编译、签名并安装完整 HAP。仅把 AOT、Wasm 或另一个 HAP 放入资源 ZIP 不能补齐这一条件。

### 通过 HDC 手动部署（高级用法）

需要排查部署问题或使用现有调试流程时，也可在应用未运行时，通过调试工具以该应用身份，将资源复制到其真实 `filesDir`：

```text
filesDir/
├── game/
│   ├── data/
│   └── b/<build>/shaders/
├── saves/                 # 存档；升级时请保留
└── config.json            # 启动后生成的设置
```

应用日志 `GameRuntime` 中的 `RESOURCE_PATH` 会打印实际目录。应用私有目录不能按普通共享存储写入；这种手动部署方式需要可调试包及对应调试权限。正常用户通过上文的文件选择器导入 ZIP 即可。游戏 `game.wasm` 已在本地编译阶段用于 AOT，运行时读取的是包内已签名 AOT 和外置数据 / 着色器。

首次通过调试工具部署资源时，可在配置好自己的本地签名后构建 debug 包：

```sh
HARMONY_BUILD_MODE=debug bash tools/build_game.sh --wasm "/absolute/path/game.wasm" --signed
```

工具默认生成 release 包；需要调试访问私有目录时，请显式设置上述 debug 模式，并确认目标设备具备相应调试权限。

部署后重启应用，等待横屏稳定后进入游戏；更换窗口比例后需重新启动。更新请覆盖安装并保留数据。正常运行前应移除自己创建的 `vulkan-buffer-probe-request.json` 等诊断请求文件，否则应用会进入独立诊断模式。

## 公开源码与 Release

源码仓库不包含游戏原始程序、游戏数据、游戏着色器或派生 AOT。Release 的完整 HAP 包含运行时和 AOT，游戏数据与匹配着色器仍需自行导入；单独构建的应用壳不具备游戏运行时或 AOT。完整 HAP 中的 AOT 是游戏程序的派生产物，其授权范围与本仓库源码的开源许可独立。

发布开源运行时二进制时，应保留 GPL 和第三方许可声明，并提供与该二进制对应的源码、修改后的 vendor 源码和构建脚本。完整对应源码见 Release 附件 `HarmonyOS-GTA-V-v0.8.0-source.tar.gz` 及同版本标签。许可见源码根目录 `LICENSE`、`THIRD_PARTY_NOTICES.md` 与 `licenses/`。华为 SDK 从官方开发环境获取，不随仓库再分发。

开发者本机的构建产物、签名配置和调试记录由忽略规则排除。

## 源码校验与私有资源测试

运行时源码编译不包含游戏着色器原文；独立 GPU 探针从用户部署的资源中读取对应文件并校验 SHA，缺失或不匹配会明确失败。包含真实私有着色器的 Rust 集成测试默认标为 ignored，需显式提供 `MUGUET_DEPTH_SHADER_FIXTURE` / `MUGUET_LUMINANCE_SHADER_FIXTURE` 路径后运行相应测试；不能把跳过这类测试当作游戏或 GPU 验证通过。

独立图形诊断的自写 WGSL 输入保存在 `native-platform/shaders/`，编译工具源码位于 `game-runtime/shader-compiler/`，不需要游戏资源。

部分历史研究脚本会比较本地 `evidence/` 中的捕获或修改前快照，例如 `game-runtime/tools/test_wsi_gpu_profile.py`；这些记录不随源码发布，也不是主应用构建的依赖。
