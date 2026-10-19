# 应用图标来源与导出

当前图标以前景居中的绿色 V 与 FIVE 飘带为主体，背景采用深黑蓝色、白色发光上半圆环和少量淡蓝涟漪。标志、圆环与水面共享蓝白轮廓光，形成统一画面。保留完整方形画布和裁切安全留白。

- 原始图：[app-icon-gta5-harmony-source.png](app-icon-gta5-harmony-source.png)，`1254 × 1254`。
- 预览 PNG：[app-icon-gta5-harmony.png](app-icon-gta5-harmony.png)，`1024 × 1024`。
- 应用资源：AppScope 与 entry 两处 `app_icon_gta5.png`，均为 `512 × 512`，内容一致。应用、主入口、独立诊断入口和启动窗口均引用 `$media:app_icon_gta5`。

使用 Lanczos3 从原始图重采样导出预览和应用资源，全部为不透明 RGB PNG。图标仅含原标志的 V 和 FIVE，背景仅留少量短蓝色涟漪。

两处应用资源应保持 SHA-256 一致。公开壳包构建脚本会核对 HAP 内图标与两处源码资源逐字节一致；构建和包校验不代表设备安装验证。

GTA V、HarmonyOS 及相关标志的版权、商标权仍归原权利人；此图标不代表 Rockstar Games、Take-Two 或华为的官方授权，软件的 GPL 许可不授予相关商标权。
