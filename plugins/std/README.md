# MutsukiStdPlugins

标准插件 owner catalog 由 `mutsuki-std-plugins::configured_std_plugin_catalog()`
提供。目前 catalog 注册内存与 SQLite 资源 Provider，以及显式配置的 Chromium
browser 插件。SQLite 资源 Provider（`mutsuki.std.resource.sqlite`）把 blob/COW
资源持久化到产品指定的数据库文件，重启后可恢复，并通过 capability 的 `delete`
命令清理一次性资源。

Browser 协议为 `mutsuki.browser.snapshot`。调用方必须预创建 schema 为
`mutsuki.browser.snapshot.output.v1` 的 COW output resource；runner 完成后写入
`{ final_url, title, html }` JSON，调用方再打开最新 descriptor。Chromium 配置必须
显式提供本机 executable、domain allowlist、timeout 与 DOM byte 上限；插件不会自动
下载浏览器，也不提供任意 JavaScript 执行面。

Mutsuki 的领域中立标准协议与插件集合。Core 只提供 runtime 机制；本仓库提供
config、database、filesystem、HTTP、observe、resource 和 workflow 能力。

## Workspace

- `protocols/`：纯协议 ID、schema 与 wire surface。
- `plugins/`：batch-first Runner、provider 与 effect gateway。

所有 Core crate 均来自固定远端 revision，仓库可脱离兄弟目录独立构建。

## Shared memory 复制边界

`ResourceAccess::SharedMemory` 表示其他进程可以按名称打开同一个 OS mapping，即
shared-addressable；它不表示 `ResourcePlanGateway::collect_read_plan` 是零复制。
`collect` 与 `inline_utf8` export 会有意返回 owned output，并受
`SharedMemoryProviderConfig::max_collect_bytes` 硬限制（默认 16 MiB）。plan 可以通过
`args.max_bytes` 请求更小的限制，但不能提高 provider 上限。

可直接消费 mapping 的调用方，在 builtin 部署中使用
`SharedMemoryResourceProvider::mapped_view`，其他进程使用
`SharedMemoryView::open(&resource_ref)`。RAII view 拥有 mapping，返回的 slice 只能在 view
生命周期内借用；`ResourceRef` 继续只携带 mapping 名称和范围，不携带 Rust pointer。
不支持 shared mapping 的平台或部署结构化返回 `resource.unsupported`，由产品显式选择
provider RPC backend。

发布后的 mapping generation 不再原地修改。COW 写先创建并提交新 mapping/generation；
只读 snapshot 复用源 mapping，不复制完整 bytes。被替换的 generation 按
`retained_generations` 保留并由 provider GC 释放；存活的 view 和 snapshot 通过自己的
RAII owner 继续保持 mapping 有效。

## Verification

```powershell
cargo fmt --check
cargo check
cargo test
cargo bench -p mutsuki-plugin-resource-shared-memory --bench shared_memory_paths
```

## Performance Model v1

Issue #5 的标准插件基准只测量真实 plugin handler/provider 边界：workflow、memory/COW、
shared-memory descriptor/open/read/release、临时 SQLite、临时文件树、loopback HTTP、
observe、permission 与 dev mock。HTTP 基准明确禁止公网请求。

```powershell
python scripts/run-performance-model.py --mode smoke --output artifacts/performance/issue5-smoke.json
python scripts/run-performance-model.py --mode reference --process-runs 3 --output artifacts/performance/issue5-reference.json
```

输出遵守 `mutsuki.performance.report/v1`，包含固定 workload/seed、owner repository
revision snapshot、稳定
output hash、p50/p95/p99/MAD、throughput、CPU/RSS、allocation、disk/network bytes 与
correctness counters。完整边界、异常判定与跨仓库使用方式见
[`docs/performance-model-issue5.md`](docs/performance-model-issue5.md)。
