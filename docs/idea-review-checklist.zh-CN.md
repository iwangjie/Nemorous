# IDEA 审阅操作对照清单

按一次 agent 改动后最常用的顺序排列。用包含已修改、未跟踪和部分暂存文件的测试仓库逐条对照。涉及快捷键的步骤使用 macOS 键位。

| 优先级 | 场景 | 验收步骤与预期 |
| --- | --- | --- |
| 1 | Commit 变更树复制 | 在 Unversioned Files 选中 `README.md`，`⌘C` 粘贴得到 `~/Projects/demo/docs/README.md`。主目录外保留绝对路径。已跟踪的变更行复制文件名。选中的行与提交勾选框各自独立。 |
| 2 | Project 文件树复制和打开 | 单击文件或目录只选中，`⌘C` 得到名称。双击或按 `Enter` 打开文件，目录箭头展开。编辑器取得焦点后，`⌘C` 复制所选文字。 |
| 3 | 从 Commit 打开差异 | 选中文件后按 `⌘D` 或双击打开 Diff。关闭 Diff 后原文件仍选中。单击是否预览按 IDEA 的对应选项分别验收。 |
| 4 | 连续看差异块和文件 | `F7` / `⇧F7` 导航差异块。到文件边界时先提示，再按一次进入下一文件首个或上一文件末个差异。`⌥←` / `⌥→` 直接切换 Diff 文件。 |
| 5 | 跳源码和往返 | 在 Diff 指定一行按 `F4`，源码定位到对应行列。`⌘[` 回到该 Diff，`⌘]` 再到源码。 |
| 6 | 明确复制路径格式 | 在树中按 `⌘⇧C` 复制选中项的绝对路径。右键「复制路径／引用」分别检查文件名、绝对路径、项目相对路径和仓库相对路径。 |
| 7 | 在 Project 中定位 | 从源码或 Diff 按 `⌥F1`，在「Select In」入口选择 Project，展开祖先目录、选中目标并滚动到可见位置。 |
| 8 | 按文件和行列定位 | `⌘⇧O` 搜索文件名或路径，粘贴 `~/.../README.md:12:3` 后回车定位。源码中 `⌘L` 打开行列跳转。 |
| 9 | 文件内查找和轻改 | 在源码用 `⌘F` 查找，`⌘G` / `⇧⌘G` 前后找匹配，`⌘R` 进行替换，`⌘Z` 撤销，`⌘S` 保存。Diff 查找作用于当前侧。 |
| 10 | 打开常用工具窗口 | `⌘0` 打开并聚焦 Commit，`⌘1` 打开并聚焦 Project。切换工具窗口时当前编辑器保持原位置。 |

后续场景包括目录范围搜索、最近文件和位置、文件历史和逐行归属、比较暂存与工作区、筛选变更和隐藏空白差异。

在本机 IDEA 2025.1.7.2 实测了 Unversioned Files 的剪贴板内容、Commit 中单击不打开 Diff，以及 Project 的单击选择和复制名称。已跟踪变更的复制格式按该版本 Commit 树的实现核对。其余快捷键和 Diff 操作以本机键位和下列 JetBrains 文档为准。[IDEA macOS 键位](https://www.jetbrains.com/help/idea/reference-keymap-mac-default.html)、[Commit 中的 Diff 行为](https://www.jetbrains.com/help/idea/commit-and-push-changes.html)、[Diff 导航规则](https://www.jetbrains.com/help/idea/comparing-file-versions.html)

Nemorous 的界面回归覆盖了上表的操作。已构建 macOS 版并在真实工作区复测单击选择、Commit `⌘C`、`⌘D`、`⌥F1`，以及切回 Commit 后保留同一个 Diff。
