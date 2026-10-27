# 应用图标来源与导出

当前安装包使用真正的分层图标：用户提供的透明 GTA V 标志作为前景，独立的不透明纯黑图片作为背景。保留原标志、文字、颜色和比例；只缩放、居中，不进行 AI 重绘，也不预先烘焙圆角。

- 用户原图：[app-icon-gta5-layered-source.png](app-icon-gta5-layered-source.png)，`911 × 800` RGBA，保留附件原始文件及透明通道。
- 黑底合成预览：[app-icon-gta5-layered-preview.png](app-icon-gta5-layered-preview.png)，`512 × 512`，模拟前后景中央 `192 × 192` 区域的合成效果。
- AppScope 与 entry 的 `resources/base/media/` 各有同一组内容完全一致的资源：`app_icon_gta5_layered.json`、`app_icon_gta5_foreground.png`、`app_icon_gta5_background.png` 和 `app_icon_gta5.png`。
- `app_icon_gta5_foreground.png` 为 `288 × 288` 透明前景；`app_icon_gta5_background.png` 为 `288 × 288`、全像素 `RGBA(0, 0, 0, 255)` 的纯黑背景。
- 应用、主入口、独立诊断入口的 `icon` 均引用 `$media:app_icon_gta5_layered`；启动窗口继续使用 `512 × 512` 黑底合成图 `$media:app_icon_gta5`。

导出采用 macOS CoreGraphics，脚本为 [`tools/export_layered_icon.swift`](../../tools/export_layered_icon.swift)。原图非透明内容边界为 `[32, 18, 878, 766)`。脚本以透明内容的中心对齐画布中心，等比缩放，将全部非透明像素置于前景中央半径约 `89` 像素的圆内，为系统蒙版与图层运动留白。原图透明度保留，字内白色和绿色 V 不作替换。

`288 × 288` 图层和中央 `192 × 192` 可见区域依据 [OpenHarmony 图标交付规范](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/design/ux-design/visual-app-icons.md)；资源描述依据 [分层图标配置](https://raw.githubusercontent.com/openharmony/docs/master/zh-cn/application-dev/quick-start/layered-image.md)。半径 `89` 是本项目的保守布局选择。桌面最终外形与动态效果由系统决定，预览不能代替设备桌面检查。

重新导出：

```sh
swift tools/export_layered_icon.swift docs/assets/app-icon-gta5-layered-source.png .
```

`tools/verify_app_icon.py --hap /path/to/package.hap` 核对两处源码资源逐字节一致，以及 HAP 内两层图片、启动合成图与源码逐字节一致。资源编译器会将分层描述中的资源名转换为数字 ID，因此校验会通过包内资源索引核对前景、背景的实际指向，并核对应用和两个入口的图标引用；游戏包和公开壳包校验均复用此检查。构建和包校验不代表设备安装验证。

旧版 `app-icon-gta5-supplied-source.png`、`app-icon-gta5-centered.png`、`app-icon-gta5-harmony-source.png` 和 `app-icon-gta5-harmony.png` 保留归档。

GTA V、HarmonyOS 及相关标志的版权、商标权仍归原权利人；此图标不代表 Rockstar Games、Take-Two 或华为的官方授权，软件的 GPL 许可不授予相关商标权。
