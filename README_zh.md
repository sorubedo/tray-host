# tray-host

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

**⚠️ 本项目由 [Claude Code](https://claude.ai/code) (DeepSeek V4 Pro) vibe-coding 生成。**

一个**无头系统托盘托管程序**（StatusNotifierItem 守护进程），专为配合 **fuzzel**、**rofi**、**dmenu** 等外部启动器使用而设计。架构灵感来源于 [cliphist](https://github.com/sentriz/cliphist) + fuzzel 的组合模式。

## 致谢

| 项目 | 角色 |
|------|------|
| [tray-tui](https://github.com/Levizor/tray-tui) | 原始 TUI 系统托盘项目 — 本项目由此裁剪而来 |
| [system-tray](https://github.com/jakestanger/system-tray) | D-Bus StatusNotifierItem 后端，承担所有底层繁重工作 |
| [cliphist](https://github.com/sentriz/cliphist) | 架构灵感来源：守护进程 + socket + 选择器 |

本项目是对 `tray-tui` 的完全重写——删除了所有 TUI/前端代码，替换为 Unix socket 守护进程 + CLI，专为与模糊选择器的可组合性而设计。

## 架构

```
D-Bus Session Bus
    │  应用程序 (Discord, Dropbox, copyq...) 注册托盘图标
    ▼
tray-host daemon (后台守护进程)
    ├── system_tray::Client → D-Bus StatusNotifierWatcher
    ├── Host → 内存托盘项缓存
    └── Unix socket → $XDG_RUNTIME_DIR/tray-host.sock
         ▲
         │
    tray-host pick    ← 你唯一需要的命令
```

## 安装

### Cargo

```
cargo install --git https://github.com/sorubedo/tray-tui
```

### 从源码编译

```
git clone https://github.com/sorubedo/tray-tui
cd tray-tui
cargo build --release
```

## 使用方法

### 1. 启动守护进程

加入 compositor 自启动，或手动运行：

```
tray-host daemon &
```

### 2. 与托盘图标交互

```
tray-host pick
```

就这么简单。`pick` 命令处理完整流程：

1. 列出托盘图标 → 启动 **fuzzel** 供选择
2. 如果图标有菜单 → 再次启动 **fuzzel** 供菜单选择
3. 通过 D-Bus 发送点击事件给应用程序
4. 如果没有菜单 → 发送默认（左键点击）激活

使用其他选择器：

```
tray-host pick --picker "rofi -dmenu -show-icons"
```

## 配置

可选配置文件位于 `$XDG_CONFIG_HOME/tray-host/config.toml`：

```toml
sorting = false   # 按标题字母顺序排列托盘项
```

## 库用法

```rust
use tray_host::Host;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = Host::new().await?;
    for (addr, title) in host.list_items() {
        println!("{addr}\t{title}");
    }
    Ok(())
}
```

## 许可证

MIT — 详见 [LICENSE](./LICENSE)。
