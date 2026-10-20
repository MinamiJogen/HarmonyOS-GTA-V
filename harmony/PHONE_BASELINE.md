# 固定手机版基线

2026-10-20 按用户选择固定正在实测的 0.8.0 版本。机器可读的二进制标识与源码哈希见 [phone-baseline.json](phone-baseline.json)。签名包仅保存在本机 `dist/HarmonyOS-GTA-V-phone-baseline-AI-signed.hap`，不纳入 Git。

当前版本包含原游戏高／中／低画质、独立超分与补帧开关；初始选择为中档并开启两项处理。中档纹理为 1，低档保留最低可运行的 ShaderQuality 1，其余画质选项最低。分辨率按真实屏幕比例计算，原生短边约 800，超分输出短边约 1080。详细选项见 [QUALITY_PRESETS.md](QUALITY_PRESETS.md)。

首页提供开始新游戏、加载存档、开始 Demo 和 ZIP 存档导入，保留原引擎保存／加载接口。格式及路径规则见 [SAVE_FORMAT.md](SAVE_FORMAT.md)。

此次冻结恢复了选定版本的原运行库和实际打包界面／桥接代码。Rust `frame-interpolation` release 库已重新编译，与原静态库 SHA-256 完全相同：`455f2ec0f03a510fae9c17ddae7d587418501281bede310048216c6e5e71c192`。完整应用重建的 `libharmony_probe.so` 与 ArkTS `modules.abc` 也均与原安装包逐字节一致。后续窗口重建、关键线程提示、等待绕过、四倍插帧等实验不属于此固定源码。

用于复现实测的图形配置见 [experiments/phone-selected-baseline.json](experiments/phone-selected-baseline.json)。这份文件只包含图形选项，不包含资源位置、签名、设备标识或存档选择。它是配置合并参考，不会被启动器自动导入；原生配置缺省的回读缓冲池关闭，实测配置明确设为 `readback_pool: true`。已有配置会保持原值，使用者应在应用完成初始配置后合并所需图形字段。其他 `experiments/` 文件是历史诊断配置，并非当前默认预设。

手机在系统性能模式 602 下，本轮 60.50 秒有效均值为 29.07fps：889 源帧＋870 新 AI 帧，排除 356 次重复呈现。旧界面 FPS 数字包含重复呈现，验收应使用 RenderService fence 唯一匹配的源帧＋AI 统计。高低档持续帧率、平板回归和所有场景平均不低于 30fps 尚未完成验收。

本版在窗口仍有效时保留已有渲染流程；窗口真正销毁后会报告需要恢复，没有包含后续同进程 Vulkan 窗口重建实验。源码能构建不等于这些未纳入功能已经完成。

原生构建使用 `MUGUET_RUNTIME_FEATURES=frame-interpolation bash game-runtime/scripts/build-ohos.sh`，应用构建沿用 [README.md](README.md) 的本地完整包流程与独立 DevEco 签名。游戏资源、AOT／模型二进制、工具链、设备日志及签名材料均保持 Git 忽略。
