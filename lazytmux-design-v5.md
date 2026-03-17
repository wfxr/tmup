# lazy.tmux — Modern Tmux Plugin Manager (v5)

## Context

TPM（tmux plugin manager）已多年未维护，存在几个结构性问题：纯 bash 实现、错误处理弱、串行安装/更新、没有 lock file、缺少现代交互式 UI。

目标是用 Rust 构建一个借鉴 lazy.nvim 设计理念的现代 tmux 插件管理器：配置简洁、安装并发、环境可复现、日常交互现代化，并在这个前提下兼容尽可能多的 TPM 插件。

v5 在 v4 的基础上又补齐了 3 个实现期容易踩坑的语义：

1. **writer-aware read path**：纯只读 `init` 不再忽略全局写锁；若检测到 writer active，则等待其完成后重新 preflight，避免读取过渡态插件目录
2. **状态与结果分离**：`list` / TUI 不再把“当前可用状态”和“最近一次 build 失败”混成单一 `status`
3. **failure marker key 含 build command hash**：如果用户修改了 `build` 命令，lazy.tmux 会把它视为新的尝试，而不是继续抑制自动重试

---

## 1. 设计目标与非目标

### 设计目标

1. **可复现**：同一份 `lazy.kdl` + `lazylock.json` 在不同机器上得到一致插件版本
2. **兼容常见 TPM 插件模式**：支持通过 `*.tmux` 入口脚本和 `@option` 工作的主流插件
3. **启动快速**：`init` 在“插件已安装且 lock 无变化”时接近零额外开销
4. **操作安全**：多 tmux client / 多 shell / TUI+CLI 并发操作时不破坏插件目录和 lock file
5. **交互现代**：TUI 为主入口，CLI 子命令面向脚本和自动化

### 非目标

1. **不复刻 TPM**：兼容的是常见插件接口，不是 TPM 的完整内部行为
2. **不复用 TPM 的旧安装目录**：从 TPM 迁移到 lazy.tmux 时允许重新 clone 插件
3. **不兼容依赖 TPM 目录布局的插件**：例如假设 `TMUX_PLUGIN_MANAGER_PATH` 的直接子目录就是插件目录的脚本
4. **不在 `init` 中隐式更新插件**：启动时允许补齐缺失插件，但不推进已有插件版本，也不重复自动尝试同一已知失败 build tuple
5. **不在 MVP 中实现 hooks / registry / 依赖解析**：这些属于后续扩展

---

## 2. 核心原则

| 原则 | 说明 |
|------|------|
| Lock-first | 有 lock file 时，默认以 lock 为准安装和恢复；只有显式 `update` 才推进版本 |
| Install 并发，Load 串行 | git 操作可并发；tmux 选项设置和插件加载按配置顺序串行执行 |
| URL-derived identity | 远程插件 id 从 canonical source 推导，避免冲突与手工命名 |
| Zero magic options | `opt-prefix` 默认空，不自动推导前缀，不自动补分隔符 |
| Compatibility by contract | 明确声明支持哪些 TPM 行为，也明确声明哪些行为不保证兼容 |
| Safe publish | 新 revision 先在 staging 目录准备 checkout，再在正式路径完成 build 与提交 |
| Writer-aware reads | 只读 init 不会在 writer active 时直接读取插件目录和加载插件 |
| 用户显式控制侵入性 | UI keybinding 默认不注册，需用户显式 opt-in |

---

## 3. 插件标识与路径模型

### 3.1 Remote plugin identity

所有远程插件的 **id** 从 source URL 规范化推导，类似 Go module path：

| source 形式 | 推导出的 id |
|---|---|
| `tmux-plugins/tmux-sensible` | `github.com/tmux-plugins/tmux-sensible` |
| `https://github.com/user/repo.git` | `github.com/user/repo` |
| `https://gitlab.com/user/plugin.git` | `gitlab.com/user/plugin` |
| `git@github.com:user/repo.git` | `github.com/user/repo` |
| `https://git.example.com/team/plugin.git` | `git.example.com/team/plugin` |

规则：

- `id` 是远程插件的**唯一主键**
- `lock key = id`
- 安装目录 = `{plugin_root}/{id}/`
- `name` 默认取 id 最后一段（basename），仅用于 TUI / 日志显示
- CLI 的主目标选择器是 `id`，不是 `name`

目录示例：

```text
~/.local/share/lazytmux/plugins/
  ├── github.com/
  │   ├── tmux-plugins/
  │   │   ├── tmux-sensible/
  │   │   ├── tmux-resurrect/
  │   │   └── tmux-yank/
  │   └── catppuccin/
  │       └── tmux/
  └── gitlab.com/
      └── user/
          └── plugin/
```

### 3.2 Local plugins

本地插件不 clone、不写入 lock file、也不参与 `install` / `update` / `restore` 这类远程版本管理命令。

语义如下：

- source 必须是本地路径
- lazy.tmux 直接原地加载
- `name` 仅用于展示
- `clean` 不会删除本地插件

### 3.3 为什么保留 full-path 布局

选择 full-path 布局的原因：

1. **天然唯一**：避免 `user1/tmux-x` 和 `user2/tmux-x` basename 冲突
2. **不需要额外 id 配置**：远程插件默认即可稳定定位
3. **lock / install path / CLI selector 一致**：文档、状态和实现更简单

代价是：`TMUX_PLUGIN_MANAGER_PATH` 不再具有 TPM 那种“直接子目录就是插件目录”的 flat layout。这个代价在 v5 中被明确纳入非目标。

---

## 4. 配置语法（KDL）

```kdl
// ~/.config/tmux/lazy.kdl

options {
    concurrency 8
    auto-install true
    auto-clean false

    // UI keybinding 默认不注册，需显式 opt-in
    bind-ui false
    ui-key "L"
}

// 最简写法
plugin "tmux-plugins/tmux-sensible"

// 指定 tag：视为 pinned release selector，update 默认跳过
plugin "tmux-plugins/tmux-yank" tag="v2.3"

// 完整配置
plugin "tmux-plugins/tmux-resurrect" {
    branch "master"
    build "make install"

    // opt-prefix 默认为 ""，key 就是完整 tmux option 名（不含 @）
    opt "resurrect-strategy-vim" "session"
    opt "resurrect-save-bash-history" "on"
}

// 设置 opt-prefix 减少重复
plugin "catppuccin/tmux" opt-prefix="catppuccin_" {
    opt "flavor" "mocha"          // -> @catppuccin_flavor "mocha"
    opt "window_text" "#W"        // -> @catppuccin_window_text "#W"
}

// 非 GitHub 源
plugin "https://gitlab.com/user/my-plugin.git"

// 本地插件（原地引用，不入 lock）
plugin "~/dev/my-tmux-plugin" local=true name="my-plugin-dev"

// 一键禁用（KDL slashdash）
/-plugin "tmux-plugins/tmux-continuum"
```

### 4.1 opt 机制

公式：

```text
set -g @{opt-prefix}{key} "{value}"
```

规则：

- `opt-prefix` 默认为 `""`
- 不自动推导 prefix
- 不自动加 `-` 或 `_`
- 用户自己对最终 tmux option 名负责

示例：

- `opt "resurrect-strategy-vim" "session"` -> `@resurrect-strategy-vim`
- `opt-prefix="catppuccin_"` + `opt "flavor" "mocha"` -> `@catppuccin_flavor`

### 4.2 插件属性一览

| 属性 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| 第一参数 | string | — | GitHub `user/repo`、完整 git URL、或本地路径 |
| `name` | string | remote: id basename；local: 路径 basename | TUI/日志显示名 |
| `opt-prefix` | string | `""` | opt key 前缀，直接拼接 |
| `branch` | string | 默认分支 | 跟踪指定分支 |
| `tag` | string | — | 命名版本；默认视为 pinned，`update` 跳过 |
| `commit` | string | — | 固定到某个 commit，`update` 跳过 |
| `local` | bool | `false` | 本地路径插件，原地引用 |
| `build` | string | — | 在最终正式目录执行；成功后本次 revision 切换才算 committed |
| `opt` | 子节点 | — | 转为 `set -g @{opt-prefix}{key} "{value}"` |

### 4.3 配置校验规则

1. 远程插件推导出的 `id` 必须唯一，否则报错并停止执行
2. `branch`、`tag`、`commit` 三者最多只能出现一个
3. `local=true` 时 source 必须解析为本地路径
4. 远程插件一律进入 lock file；本地插件一律不进入 lock file
5. 本地插件不能声明 `branch` / `tag` / `commit`

---

## 5. 命令语义

### 5.1 命令总览

```text
lazytmux                    # 进入 TUI（主入口）
lazytmux init               # tmux 启动时调用：补齐缺失插件、应用选项、加载插件
lazytmux install [id]       # 安装所有/指定缺失远程插件
lazytmux update [id]        # 更新所有/指定远程插件；唯一推进版本并重写 lock 的命令
lazytmux clean              # 移除未声明的托管远程插件
lazytmux restore [id]       # 恢复到 lock 记录的 commit
lazytmux list               # 列出插件状态（CLI 纯文本）
lazytmux migrate            # 从 .tmux.conf 迁移 TPM 声明
```

说明：

- CLI 的主目标选择器是 **remote plugin id**
- `name` 只用于展示，不作为稳定 CLI 主键
- 本地插件不参与 `install` / `update` / `restore`

### 5.2 `init`（tmux 启动路径）

必须满足“快”和“稳”两个条件。

```text
1. 只读 preflight：
   - 定位并解析 lazy.kdl
   - 读取 lazylock.json（若存在）
   - 校验配置合法性
   - 扫描已安装远程插件目录
   - 读取 build failure markers
   - 计算本次 plan 是否需要写状态
2. 若 plan 为纯只读：
   - 不获取独占锁
   - 若检测到当前存在 writer active，则等待 writer 完成后，从步骤 1 重新开始 preflight
   - 若不存在 writer active，则设置 TMUX_PLUGIN_MANAGER_PATH，并按配置声明顺序应用 opt 和加载 *.tmux
3. 若 plan 需要写状态：
   - 获取全局操作锁
   - 在锁内重新扫描、重新读取 failure markers、重新计算 plan
   - 若 auto-install=true，仅安装缺失远程插件：
     - 有 lock entry -> 按 lock 中的 commit 安装
     - 无 lock entry -> 按配置解析版本，安装后新增 lock entry
     - 若 `(plugin id, commit, build command hash)` 已有已知 build 失败记录，则 init 默认不自动重试
   - 若 auto-clean=true，移除未声明的托管远程插件
   - 设置 TMUX_PLUGIN_MANAGER_PATH
   - 按配置声明顺序应用 opt 并加载 *.tmux
   - 若 bind-ui=true 且目标键未占用，注册 UI keybinding
   - 释放全局操作锁
```

关键约束：

- `init` **不更新已安装插件到更新版本**
- `init` **不因远程 HEAD 变化而改写已有 lock**
- `init` **不会在 writer active 时无锁读取插件目录**
- `init` **不自动重试同一 `(plugin id, commit, build command hash)` 的已知失败 build**
- 所有插件已安装且 lock 无变化时，只做轻量 source，不做 git 网络访问

### 5.3 `install [id]`

- 补齐缺失远程插件，不修改已安装插件版本
- 有 lock entry -> 安装 lock 指定 revision
- 无 lock entry -> 按配置解析 revision，安装后写入 lock
- 已安装插件默认跳过
- 显式 `install` 可覆盖 init 的“已知失败 build 不自动重试”保护

### 5.4 `update [id]`

`update` 是**唯一推进 lock** 的命令。

revision policy：

- `branch` -> fetch 后更新到远端分支最新 commit
- 未指定 `branch/tag/commit` -> 跟踪默认分支最新 commit
- `tag` -> 视为 pinned release selector，默认跳过并报告 `pinned-tag`
- `commit` -> 固定版本，跳过并报告 `pinned-commit`

说明：如果未来需要“可移动 tag”的策略，可以单独设计新的 policy；v5 不引入这类额外语义。

### 5.5 `restore [id]`

- 将远程插件 checkout 到 lock 中记录的 commit
- 前提：lock 中存在对应 entry
- 插件缺失时先按 lock 重装再 checkout
- 如果实际 revision 发生变化，且插件声明了 `build`，则 **restore 后也执行 build**

### 5.6 `clean`

- 移除托管目录中已安装但未声明的远程插件
- 只清理 lazy.tmux 托管的远程目录
- 不删除本地插件 source
- 可在删除后顺手清理空的中间父目录

### 5.7 `list`

输出至少包含：

- `id`
- `name`
- `source`
- `current commit`
- `lock commit`
- `kind`：`remote` / `local`
- `state`：`installed` / `missing` / `outdated` / `pinned-tag` / `pinned-commit` / `unmanaged`
- `last-result`：`ok` / `build-failed` / `none`

示例：

- update 后 build 失败并成功回滚：`state=installed`, `last-result=build-failed`
- fresh install build 失败：`state=missing`, `last-result=build-failed`

### 5.8 `migrate`

- 从 `.tmux.conf` 提取 `set -g @plugin` 和相关 `set -g @xxx` 选项
- 无法可靠推断 `opt-prefix` 时，不做猜测，生成 TODO 注释
- 不覆盖现有 `lazy.kdl`
- 迁移结果面向 lazy.tmux 语义，不保证保留 TPM 的目录布局假设

---

## 6. Lock File

### 6.1 格式

```json
{
  "version": 1,
  "plugins": {
    "github.com/tmux-plugins/tmux-sensible": {
      "source": "tmux-plugins/tmux-sensible",
      "tracking": { "type": "branch", "value": "main" },
      "commit": "abc1234567890abcdef1234567890abcdef1234"
    },
    "github.com/tmux-plugins/tmux-resurrect": {
      "source": "tmux-plugins/tmux-resurrect",
      "tracking": { "type": "branch", "value": "master" },
      "commit": "def5678901234567890abcdef1234567890abcd"
    },
    "github.com/catppuccin/tmux": {
      "source": "catppuccin/tmux",
      "tracking": { "type": "branch", "value": "main" },
      "commit": "1234567890abcdef1234567890abcdef12345678"
    }
  }
}
```

### 6.2 语义

- lock key = remote plugin `id`
- `init` / `install`：有 lock -> 按 lock 安装；无 lock -> 解析 revision 并新增 lock entry
- `update`：唯一推进 lock 的操作
- `restore`：严格以 lock 中的 commit 为目标
- 本地插件不入 lock；远程插件始终入 lock
- 部分失败时：成功完成发布的插件更新 lock，失败的保持旧值

### 6.3 写入策略

- 写入 `lazylock.json.tmp`
- `fsync`
- `rename` 为 `lazylock.json`
- 仅在当前操作成功提交后统一写入

---

## 7. 并发与发布模型

### 7.1 全局操作锁

所有写操作都需获取全局互斥锁：

- `init`
- `install`
- `update`
- `restore`
- `clean`
- TUI 中对应的写操作

`list` 和纯展示型 TUI 刷新不需要。

锁文件：

```text
$XDG_STATE_HOME/lazytmux/operations.lock
```

### 7.2 Staging

所有远程插件 revision 切换都先在 staging 目录完成。为了保证 publish protocol 可以依赖同一 filesystem 内的 `rename`，`plugins/`、`.staging/`、`.backup/` 必须位于同一个 XDG data root 下：

```text
{data_dir}/plugins/
{data_dir}/.staging/
{data_dir}/.backup/
```

staging 目录命名示例：

```text
{data_dir}/.staging/{id-hash}-{pid}-{nonce}/
```

在 staging 中完成：

1. clone / fetch
2. checkout 目标 revision
3. 校验 staging 目录可发布

### 7.3 Publish protocol

v5 不再把“对已有非空目录直接 rename 覆盖”当作设计前提；发布协议区分两种情况。

#### Fresh install

若正式目录不存在：

1. `rename(staging, target)`
2. 在最终 `target` 目录执行 `build`（若声明且此次 revision 实际变化）
3. 若第 2 步失败，则删除失败的 `target`

#### Replace existing plugin

若正式目录已存在：

1. `rename(target, backup)`
2. `rename(staging, target)`
3. 在最终 `target` 目录执行 `build`（若声明且此次 revision 实际变化）
4. `build` 成功后删除 `backup`
5. 若第 2 步或第 3 步失败，则回滚：删除失败的 `target`，再 `rename(backup, target)`

说明：

- 这不是无锁的“单系统调用原子覆盖”
- 但在全局操作锁保护下，对 lazy.tmux 自身的读写已经足够安全
- 相比“直接 rename 覆盖非空目录”，这是可实现且可回滚的协议

### 7.4 Lock file 提交时机

- 插件目录发布并在最终目录 build 成功后，才允许更新对应 lock entry
- 所有成功发布的插件完成后，再统一原子写入 `lazylock.json`
- 如果进程中途失败，未发布成功的插件不应污染 lock

### 7.5 崩溃恢复

启动时可清理过期 staging 目录，但必须遵循：

- 只清理 lazy.tmux 命名规则的临时目录
- 不删除正式插件目录
- 遇到残留 backup 时优先保守处理：报警而不是自动猜测覆盖

### 7.6 Build failure markers

当最终目录中的 `build` 失败时，lazy.tmux 记录 failure marker，至少包含：

- plugin `id`
- target commit
- build command hash
- 失败时间
- `build` 命令
- stderr 摘要

failure marker 的抑制 key 为：

```text
(plugin id, target commit, build command hash)
```

语义：

- `update` / `restore` 失败时：回滚到旧版本，lock 保持旧值
- `fresh install` 失败时：删除失败目录，状态回到 `missing`
- `init` 遇到同一 `(plugin id, commit, build command hash)` 的已知失败记录时，默认只报警，不自动重试
- 用户显式执行 `install` / `update` / `restore` 时，可再次尝试
- 同一插件成功完成一次 build 后，应清除匹配的 failure marker
- 若用户修改了 `build` 命令，则 build command hash 变化，视为新的尝试

---

## 8. TPM 兼容契约

### 8.1 兼容目标

v5 的兼容目标是：

> 兼容“通过 tmux options 配置、通过一个或多个 `*.tmux` 文件作为入口脚本加载”的 TPM 插件。

换句话说，兼容面聚焦在：

- `tmux set -g @...` 选项
- `TMUX_PLUGIN_MANAGER_PATH`
- 执行插件目录中的 `*.tmux` 文件

### 8.2 明确支持的行为

| 契约 | 要求 |
|------|------|
| 环境变量 | `tmux set-environment -g TMUX_PLUGIN_MANAGER_PATH "{plugin_root}/"`，带 trailing slash |
| 选项设置 | 在 source 前通过 `tmux set -g @key "value"` 设置 |
| 执行范围 | 执行插件目录下所有 `*.tmux` 文件 |
| 单插件内顺序 | `*.tmux` 按文件名字典序 |
| 插件间顺序 | 按 `lazy.kdl` 声明顺序串行 |
| 本地插件 | 同样应用 opt 并执行其目录下 `*.tmux` 文件 |

补充说明：

- `TMUX_PLUGIN_MANAGER_PATH` 指向的是 lazy.tmux 的插件根目录
- 该根目录内部布局遵循 lazy.tmux 的 full-path 规则，而不是 TPM 的 flat layout

### 8.3 明确不保证兼容的行为

以下行为**不在兼容承诺内**：

1. **依赖 TPM flat layout**
   - 假设 `TMUX_PLUGIN_MANAGER_PATH` 的直接子目录就是插件目录
   - 通过 `ls "$TMUX_PLUGIN_MANAGER_PATH"` 枚举插件名
   - 通过 basename-only 规则推导 peer plugin 路径

2. **依赖 TPM repo 或 helper 脚本**
   - 直接调用 TPM 的内部 shell helper
   - 检测 TPM repo 是否存在
   - 假设 `~/.tmux/plugins/tpm/` 或同类目录存在

3. **依赖 TPM 快捷键工作流**
   - 依赖 `prefix + I`
   - 依赖 `prefix + U`
   - 依赖 TPM 的 clean/update prompt 行为

### 8.4 兼容声明的实际含义

v5 的意思不是“任何支持 TPM 的插件都无修改可运行”，而是：

- 如果插件把 TPM 当作一个“负责设置 tmux 选项并执行 `*.tmux` 文件的加载器”，那么它大概率可以工作
- 如果插件把 TPM 当作一个“带特定目录布局和内部 helper 的平台”，那么不保证兼容

这个边界是**有意为之**，不是遗漏。

---

## 9. tmux 集成

### 9.1 推荐集成方式

用户在 `.tmux.conf` 中显式加入：

```tmux
run-shell "lazytmux init"

# tmux 3.2+
bind L display-popup -E -w 80% -h 80% "lazytmux"

# 旧版本可手动改为：
# bind L split-window -h "lazytmux"
```

### 9.2 自动注册 keybinding（opt-in）

若配置：

```kdl
options {
    bind-ui true
    ui-key "L"
}
```

则 `init` 可以尝试注册快捷键，但必须遵循：

1. 仅在 tmux 会话内执行
2. 仅在目标键未被占用时注册
3. tmux >= 3.2 注册 popup，旧版本注册 split-pane
4. 按键已占用时仅输出 warning

---

## 10. TUI

### 10.1 布局

```text
╭──────────────────── lazy.tmux ─────────────────────╮
│                                                     │
│  Installed 12   Updates 3   Missing 1   Pinned 2   │
│                                                     │
│  ✓ tmux-sensible      main   abc1234   locked       │
│  ↻ resurrect          main   def5678   update       │
│  ! catppuccin/tmux    main   ----      missing      │
│  • my-plugin-dev      local  ----      local        │
│                                                     │
│  I install  U update  C clean  R restore  / search  │
│  l log  d diff  x remove  ? help  q quit            │
╰─────────────────────────────────────────────────────╯
```

### 10.2 功能

- 实时并发操作进度
- 颜色编码 `state`，并额外显示 `last-result` / warning
- 展开详情：`id`、source、commit log、diff 摘要、配置选项
- 键盘驱动，单键操作
- `/` 搜索过滤
- 后端写锁被其他进程持有时显示 `busy`

---

## 11. 目录结构

```text
~/.config/tmux/lazy.kdl                 # 用户配置
~/.config/tmux/lazylock.json            # lock file（纳入版本控制）

~/.local/share/lazytmux/
  ├── plugins/
  │   ├── github.com/tmux-plugins/tmux-sensible/
  │   ├── github.com/catppuccin/tmux/
  │   └── gitlab.com/user/plugin/
  ├── .staging/                          # staging 目录（与 plugins 同盘）
  └── .backup/                           # publish 回滚目录（与 plugins 同盘）

~/.local/state/lazytmux/
  ├── operations.lock                   # 全局操作锁
  └── failures/                         # build failure markers
```

配置文件查找顺序：

1. `$LAZY_TMUX_CONFIG`
2. `$XDG_CONFIG_HOME/tmux/lazy.kdl`
3. `~/.config/tmux/lazy.kdl`
4. `~/.tmux/lazy.kdl`

---

## 12. 技术栈

| 组件 | 选择 | Crate |
|------|------|-------|
| 配置解析 | KDL | `kdl` |
| Lock file | JSON | `serde_json` |
| CLI | clap | `clap` (derive) |
| 异步运行时 | tokio | `tokio` |
| TUI | ratatui | `ratatui` + `crossterm` |
| Git 调用 | shell out | `tokio::process::Command` |
| 错误处理 | — | `anyhow` + `thiserror` |
| 文件锁 | OS flock | `fd-lock` |
| 临时目录 | tempfile | `tempfile` |
| XDG 路径 | — | `etcetera` |

---

## 13. 项目结构

```text
lazytmux/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI 入口：无参数 -> TUI，有子命令 -> 对应操作
│   ├── config.rs            # KDL 配置解析与校验
│   ├── model.rs             # PluginSpec / LockEntry / OperationPlan 数据结构
│   ├── planner.rs           # 从 config + lock + installed state 计算目标状态
│   ├── plugin.rs            # install/update/restore/clean 核心流程
│   ├── git.rs               # clone/fetch/checkout
│   ├── loader.rs            # set-option + source 所有 *.tmux（按声明顺序）
│   ├── lockfile.rs          # lazylock.json 读写（atomic write）
│   ├── state.rs             # 操作锁、staging、publish、failure markers
│   ├── tmux.rs              # tmux 命令封装
│   ├── ui/
│   │   ├── mod.rs           # TUI 主界面 + 事件循环
│   │   ├── plugin_list.rs   # 插件列表
│   │   ├── progress.rs      # 任务进度
│   │   └── detail.rs        # 详情视图
│   └── error.rs             # 错误类型
└── tests/
    ├── config_test.rs       # 配置解析测试（含校验规则）
    ├── planner_test.rs      # lock-first 语义测试
    ├── lockfile_test.rs     # lock file 读写测试
    ├── state_test.rs        # 发布协议与并发安全测试
    └── integration/         # 端到端集成测试
```

---

## 14. Roadmap

### Phase 1: Core Engine

- [ ] KDL 配置解析与校验
- [ ] URL -> id 路径推导
- [ ] planner：config + lock + installed state -> 目标状态
- [ ] preflight + replan init 路径
- [ ] staging + publish protocol
- [ ] lock file 生成、读取与原子更新
- [ ] 全局操作锁
- [ ] build failure marker 机制
- [ ] CLI 输出（list）

### Phase 2: tmux Integration

- [ ] `init` 命令
- [ ] opt 应用（`set -g @...`）
- [ ] `*.tmux` 加载与声明顺序保证
- [ ] `TMUX_PLUGIN_MANAGER_PATH` 设置
- [ ] `restore` / `clean` 命令

### Phase 3: TUI

- [ ] ratatui 交互界面
- [ ] tmux popup / split-pane 集成
- [ ] 实时进度显示
- [ ] 详情视图（id / source / log / diff / build output）
- [ ] busy 状态检测

### Phase 4: Advanced Features

- [ ] Hook 系统
- [ ] 条件加载
- [ ] 显式依赖声明
- [ ] `migrate` 命令
- [ ] 插件模板 / 脚手架

---

## 15. 验证计划

1. 创建项目骨架，`lazytmux --help` 与 `lazytmux list` 可运行
2. 配置解析测试：GitHub shorthand、git URL、本地路径、id 冲突、branch/tag/commit 互斥
3. URL -> id 推导测试：覆盖 GitHub/GitLab/自建 Git、ssh URL、`.git` 去后缀
4. Lock-first 语义测试：新机器有 lock -> 按 lock 安装；无 lock -> 生成 lock；只有 update 改写 lock
5. 发布协议测试：
   - fresh install 成功发布
   - existing plugin replace 成功切换
   - replace 中途失败可回滚
6. build 语义测试：
   - build 在最终目录执行
   - update / restore build 失败时回滚到旧版本
   - fresh install build 失败时回到 `missing`
   - init 不自动重试同一已知失败 `(id, commit, build command hash)`
   - 修改 build 命令后会触发新的尝试
7. 并发安全测试：两个进程同时 init / update，确认 preflight + replan、writer-aware read path、文件锁与 lock file 提交正确
8. TPM 兼容测试：安装 tmux-resurrect，验证所有 `*.tmux` 执行、`@option` 正确设置
9. 非兼容边界测试：构造一个依赖 flat layout 的插件，验证文档声明的“不保证兼容”行为
10. 真实 tmux 环境验证：`run-shell "lazytmux init"` 启动开销、popup 回退、自动绑定不覆盖已有按键
11. 端到端：空环境 init -> install -> 验证插件生效 -> update -> restore -> clean
