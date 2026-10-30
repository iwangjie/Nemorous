# Nemorous 插件开发

Nemorous 的插件是一个带 `plugin.json` 清单的目录，逻辑写在 JavaScript 中，运行在应用内置的 goja VM 里。插件通过宿主提供的声明式 UI 树绘制工具窗口，不接触应用内部 Go 结构，也不能直接访问文件系统或启动进程：这些能力必须由清单声明权限，再由宿主 API 提供。

官方 Maven 插件位于 [`plugins/maven`](../plugins/maven)，可直接作为示例。

## 目录结构

```
my-plugin/
  plugin.json      # 清单
  main.js          # 入口（manifest.main）
  icon.svg         # 可选，manifest.icon
```

将目录打成 zip（`plugin.json` 位于根目录，或仅有一层包装目录），即可通过「插件市场 → 从文件安装」安装。

## 清单

```json
{
  "id": "maven",
  "name": "Maven",
  "version": "1.0.0",
  "description": "IDEA 风格的 Maven 工具窗口",
  "author": "Nemorous",
  "homepage": "https://github.com/iwangjie/Nemorous",
  "minAppVersion": "0.3.0",
  "main": "main.js",
  "icon": "icon.svg",
  "permissions": ["workspace.read", "process.exec"],
  "contributes": {
    "panels": [{ "id": "maven", "title": "Maven", "icon": "maven" }],
    "commands": [
      { "id": "maven.reload", "title": "Maven: 重新发现项目", "action": "refresh" }
    ]
  }
}
```

| 字段 | 说明 |
| --- | --- |
| `id` | 小写字母/数字/`-`，安装目录名与状态键 |
| `version` | `x.y.z`，市场据此判断更新 |
| `minAppVersion` | 低于该 Nemorous 版本时市场禁用安装 |
| `main` | 包内 `.js` 路径，禁止 `..` 与绝对路径 |
| `permissions` | `workspace.read`、`process.exec`；未声明则对应 API 抛错 |
| `contributes.panels` | 右侧工具窗口，最多 8 个 |
| `contributes.commands` | 命令面板条目，`action` 会传给 `handleAction` |

安装包总大小上限 32 MB，单文件 8 MB，最多 512 个条目；符号链接与越界路径会被拒绝。

## 生命周期

`main.js` 在插件启用时执行一次。宿主会调用以下全局函数：

```js
function activate() {}                     // 可选
function render(panelId) { return uiTree; } // 必需
function handleAction(action, payload, panelId) {} // 可选
function onWorkspaceChanged() {}            // 可选
function deactivate() {}                    // 可选
```

- `render` 必须返回一个 UI 树；宿主在状态变化后重新调用，并缓存结果。
- 面板被打开时宿主会派发 `"open"` 动作；用户点击标题栏刷新图标派发 `"refresh"`。
- 工作区加载或切换后调用 `onWorkspaceChanged`，随后若面板打开会再派发 `"open"`。
- 单个动作/渲染抛出异常时，面板显示错误，其他插件不受影响。

## 宿主 API

全局对象 `nemorous`：

| API | 说明 |
| --- | --- |
| `nemorous.appVersion` | 当前 Nemorous 版本 |
| `nemorous.plugin` | `{id,name,version,dir}` |
| `nemorous.workspace()` | `{path, repos:[{id,name,path}]}` |
| `nemorous.fs.read(path)` | 读取工作区内文本，≤4 MB |
| `nemorous.fs.list(path)` | `[{name,path,dir,size}]` |
| `nemorous.fs.exists(path)` | 布尔值 |
| `nemorous.fs.resolve(base, rel)` | 解析相对路径并校验边界 |
| `nemorous.exec(opts)` | 异步启动进程，返回 job id |
| `nemorous.cancel(jobId)` | 终止进程组 |
| `nemorous.storage.get/set/remove` | 插件私有 JSON 存储（≤1 MB，跨升级保留） |
| `nemorous.notify([kind,] message)` | 右下角提示，kind 为 `ok/warn/err/info` |
| `nemorous.invalidate()` | 请求重绘（宿主限流到 80 ms） |

`exec` 选项：

```js
var id = nemorous.exec({
  command: "mvn",
  args: ["-B", "-ntp", "package"],
  cwd: nemorous.workspace().path,
  env: { MAVEN_OPTS: "-Xmx1g" },       // 可选
  onOutput: function (chunk) { ... },  // 合并的 stdout/stderr，按行
  onExit: function (code, message) { ... }
});
```

`cwd` 必须位于工作区、其仓库或插件目录内；命令可执行文件不会再被限制（`process.exec` 权限即代表可运行任意程序），安装前请核对市场展示的权限。

## 声明式 UI

`render` 返回节点对象；宿主用原生控件绘制，因此插件不能自定义颜色与任意布局。

```js
{
  type: "column",             // column | row | text | button | input | checkbox |
                              // list | code | badge | divider | spacer | spinner
  grow: true,                 // 占据剩余空间
  gap: 8, padding: 10,
  scroll: true,               // 容器滚动
  children: [ ... ]
}
```

| 类型 | 主要字段 |
| --- | --- |
| `column` / `row` | `children`, `gap`, `padding`, `grow`, `scroll`, `maxHeight`, `width` |
| `text` | `text`, `size`, `bold`, `muted`, `mono`, `tone`, `grow` |
| `button` | `text`, `action`, `payload`, `primary`, `active`, `small`, `disabled` |
| `input` | `key`, `value`, `placeholder`, `action`, `grow`, `width`, `autoFocus` |
| `checkbox` | `key`, `text`, `checked`, `action`, `payload` |
| `list` | `items:[{text,detail,depth,action,payload,tone,selected,bold,mono}]`, `grow`, `scroll`, `maxHeight` |
| `code` | `text`, `grow`, `scroll`, `follow`（输出增长时跟随末尾）, `maxHeight` |
| `badge` | `text`, `tone` (`accent/ok/warn/err/muted`) |
| `spacer` / `divider` / `spinner` | `text`（spinner） |

- `input` 的值由宿主保留，输入时派发 action（默认 `"input"`）并携带 `{key,value}`。
- `checkbox` 派发携带 `{key,checked}`；插件更新状态后重绘即可。
- 按钮/列表项点击派发 `action` 与 `payload`。
- 单棵树最多 4000 个节点、24 层、4 MB 文本；`code` 文本超出时保留末尾。

## 市场索引与签名校验

官方索引是仓库中的 [`plugins/registry.json`](../plugins/registry.json)，应用内嵌一份并默认从 `main` 分支拉取最新版。远程条目需要 `downloadUrl` 与 `sha256`，下载后校验哈希；`builtin: true` 表示包已随应用发布，由内置副本安装。

```json
{
  "schema": 1,
  "plugins": [{
    "id": "maven", "name": "Maven", "version": "1.0.0",
    "main": "main.js", "minAppVersion": "0.3.0",
    "builtin": true,
    "tags": ["java", "maven"], "official": true
  }]
}
```

用户可在「插件市场」底部修改索引地址，或点击「恢复官方」。

## 安装位置与状态

- macOS: `~/Library/Application Support/nemorous/plugins/`
- Linux: `~/.config/nemorous/plugins/`
- Windows: `%APPDATA%\nemorous\plugins\`

`state.json` 记录启用状态与来源；`installed/<id>/` 是包文件，`data/<id>/storage.json` 是插件存储。卸载会同时删除两者。
