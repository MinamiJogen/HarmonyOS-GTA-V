# 图形 PSO 记录与加载预热

运行时在真正记录 `draw*` 后保存该管线的完整组合，下次启动在 GPU 命令环就绪前提前构建，并保留管线供首次绘制复用。OHOS 上所有管线创建仍在 GPU 线程串行执行。

实现位于 `game-runtime/src/gpu.rs` 和 `game-runtime/src/pso_warmup.rs`。它遵循 Khronos [Resource Cache Warmup](https://docs.vulkan.org/samples/latest/samples/performance/pipeline_cache/README.html#_resource_cache_warmup) 的记录与重建思路。

## 保存和匹配

配方包括实际 VS/PS 的资源哈希、暂停地图变体标志、变换后源码摘要、VS 输入签名、顶点元素与有效 stride、图元与 strip 索引格式、混合/深度/光栅状态、实际颜色及深度附件格式、采样数、深度/模板只读标志、纹理及采样器过滤掩码。

运行期对象 ID、附件尺寸、视口、裁剪框、blend constant、stencil reference 和资源绑定不会进入持久化身份。预热失败或没有匹配组合时，保留正常的首次使用编译。

配方保存在应用文件目录的 `native-pso-warmup-v1/recipes.json`。版本、运行时及 wgpu 源码、资源版本与 shader index、实际适配器、驱动、特性和限制共同决定身份；单个着色器还检查源码摘要。JSON 带 SHA-256 校验，损坏或身份不符时跳过读取。

最多记录并额外保留 4096 个组合，文件上限 16 MiB。新增记录经过容量检查和去重，后台线程负责序列化与原子写入，写入快照至少间隔 2 秒。已有绘制缓存沿用原有生命周期。GPU 内部管线内存开销与驱动有关，当前只限制额外保留的条数。

当前预热覆盖客体图形绘制 PSO。计算管线和内部转换/呈现管线继续使用各自的创建路径。现有 `pipeline_cache` 开关独立控制驱动二进制缓存，开启它可以配合配方预热使用。

## 配置与诊断

| config.json | 默认 | 行为 |
|---|---|---|
| `pso_warmup` | `true` | 记录完整组合并在下次加载时提前构建；`false` 关闭记录与预热 |
| `pso_warmup_read_only` | `false` | 读取已有配方，但不创建或改写缓存文件 |

`pipeline_cache_read_only=true` 同时禁止配方写入，方便维持现有只读实验约束。配置仅在新进程启动时应用。独立运行时也可使用 `MUGUET_PSO_WARMUP=0`、`MUGUET_PSO_WARMUP_READ_ONLY=1`。

日志 `pso-warmup loading complete` / `pso-warmup` 提供：

- `loaded`、`built`、`rejected`、`failed`：读取、构建和回退情况。
- `loadingWallNs`：加载阶段预热的主机耗时。
- `warmLookupHits`、`warmFirstDraws`：缓存查找复用和首次真正绘制的预热管线数。
- `drawCompileCalls`、`drawCompileWallTotalNs`、`drawCompileWallMaxNs`：仍在首次绘制发生的编译调用及主机耗时。
- `retained`、`limit`：额外保留的组合数和上限。

启用 `frame_interpolation_slow_trace=true` 时，还可在 `gpu-slow-operations-<pid>.jsonl` 检查超过跟踪阈值的 `pso_loading_warmup`、`pso_warmup_compile`、`pso_draw_miss` span；开启 CPU/帧间隔跟踪可对照 `pipelineCreate` 阶段。统计值用于确认工作是否转移到加载期，停顿改善需要帧间隔实测。

## 本地验证

```sh
python3 tools/test_pso_warmup.py
python3 tools/test_pso_pipeline_replay.py
python3 game-runtime/tools/test_empty_init_transition.py
python3 tools/test_prepare_native_trial.py
python3 tools/test_run_builtin_benchmark.py
python3 tools/test_analyze_native_benchmark.py
bash game-runtime/scripts/build-ohos.sh
```

存储测试编译真实生产模块。管线测试提取生产描述符、加载、查找和编译方法，在主机 wgpu NOOP 后端验证管线与显式绑定布局；着色器加载/变换使用小型测试资源。NOOP 验证覆盖 CPU 描述符及命令合法性。

## 真机验收安排

1. 保持包、画质、分辨率、帧队列和其他性能开关一致。先运行一遍固定路线，覆盖首次进入、转动镜头及场景切换，确认配方已写入。首次运行用于收集组合。
2. 保留相同配方文件，分别用 `pso_warmup=false` 和 `pso_warmup=true,pso_warmup_read_only=true` 冷启动，交替多次重复相同路线。若启用驱动二进制缓存，两组固定相同数据与只读策略。
3. 同时比较加载耗时、进入世界后的源帧间隔 p95/p99/最大值、超过 100 ms/500 ms/1 s 的停顿次数及管线创建主机耗时；单独观察首次进入、转镜头和切场景窗口。
4. 确认预热组 `built` 和 `warmFirstDraws` 大于零、首次绘制编译减少、画面和输入正常，并记录进程内存开销。新增组合在下一次启动才能预热。

内置 benchmark 的原有工具已支持 `pso_warmup` 对照及 `pso_warmup_read_only` 固定条件；它与手动场景路线分别留存证据。
