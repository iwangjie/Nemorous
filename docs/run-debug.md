# 运行、测试与调试

Nemorous 的运行面板把 Run / Test / Debug 统一成可保存的配置，输出、退出码与调试会话都在底部面板中展示。

## 配置字段

| 字段 | 说明 |
| --- | --- |
| Name | 显示名称 |
| Kind | `run`、`test`、`debug` |
| Command | 可执行文件；Windows 的 `.cmd/.bat` 会自动经 `cmd.exe` 启动 |
| Args | 参数，支持空格分隔的引号，如 `-run "Test A"` |
| Cwd | 工作目录，相对工作区或绝对路径；`run/test` 默认 `.` |
| Env | 每行一个 `K=V` |
| Adapter / AdapterArgs | Debug：DAP 适配器命令与参数 |
| Network / Address | Debug：`stdio`（默认）或 `tcp`，TCP 时填写 `host:port` |
| Program / ProgramArgs | Debug：传给 DAP `launch` 的 `program` 与 `args` |
| Launch | 合并进 DAP `launch` 的额外 JSON，如 `{"mode":"debug"}` |

## 调试适配器示例

### Go（Delve）

1. 安装 `dlv`。
2. 新建 Debug 配置：
   - Adapter: `dlv`
   - AdapterArgs: `dap --listen=127.0.0.1:38697`
   - Network: `tcp`
   - Address: `127.0.0.1:38697`
   - Program: `./cmd/app`（或包路径）
   - Launch: `{"mode":"debug"}`
3. 在代码行号栏点击设置断点（调试配置选中时），运行配置；命中后可在「调用栈」「变量」中查看并单步。

### Python（debugpy）

- Adapter: `python3`
- AdapterArgs: `-m debugpy --listen 127.0.0.1:5678 --wait-for-client`
- Network: `tcp`
- Address: `127.0.0.1:5678`
- Program: 入口脚本

不同适配器的必填 launch 参数不同；用 `Launch` 原样传入。Nemorous 实现的是 DAP 客户端与面板，不内置某种语言的调试器。

## 断点与测试

- 断点按 `仓库::文件` 持久化，在编辑器行号栏点击切换；调试启动时发送给适配器。
- Test 配置就是一条测试命令（如 `go test ./...`、`npm test`、`mvn test`）；输出和退出码直接显示，失败时状态为红色。
- Run 面板可同时保存多个配置，切换后点「运行」执行；Stop 会终止整个进程组。
