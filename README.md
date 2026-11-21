# 齐刻 Qike Flash

多 U 盘 / SD 卡批量烧录工具。一块镜像，同时写入多块设备，写完后逐盘无缓存回读校验，全程台账可查。

- 版本：0.0.1-alpha
- 平台：Windows
- 协议：[Apache-2.0](LICENSE)
- 作者：[youye-luna](https://github.com/youye-luna)

## 功能特性

- **批量并发写入**：一次选择多块 USB / SD 卡，扇出式并发烧录，每块盘独立显示状态与进度。
- **拖拽加载镜像**：支持 `.img` / `.iso` 等镜像文件，兼容 `.xz` 压缩镜像，拖入窗口即可加载。
- **写入前清零**：写入前清零目标盘前 1MB，清除残留 MBR/GPT 签名，避免分区表混淆。
- **无缓存回读校验**：校验绕过系统缓存（`FILE_FLAG_NO_BUFFERING`）读回比对，确保结果真实有效。
- **安全的设备身份**：设备 ID 由序列号 + 容量 + 端口路径派生，界面、日志、台账中不出现 `PhysicalDriveN`；确认到写入之间按设备 ID 重新枚举比对，杜绝盘号漂移（TOCTOU）。
- **整批原子性**：任一盘打开失败或容量不足，整批不写；单盘写失败不影响其他盘，但最终状态必为「已中止」，失败盘明确可见。
- **三语界面**：简体中文 / 繁體中文 / English 一键切换，设置持久化保存。
- **日志与台账**：运行日志存放在安装目录 `logs`，操作台账存放在安装目录 `history`，失败自动展开日志，支持双击复制。

## 技术栈

- Rust（stable）+ egui / eframe 图形界面
- Windows API（SetupAPI 枚举、锁卷写入、无缓存 IO）
- xz2 / sha2 / serde / chrono
- 编译为 Windows GUI 子系统，启动无控制台窗口

## 使用

图形界面：

```bash
multi-burn
```

命令行：

```bash
multi-burn --list    # 列出可写入的 USB / SD 卡候选
multi-burn --help    # 查看帮助
```

## 源码构建

```bash
cargo build --release
```

## 仓库

GitHub：<https://github.com/youye-luna/qike-flash>
