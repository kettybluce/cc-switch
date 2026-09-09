# Windows MSI Error 5（拒绝访问）

本仓库的 WiX 模板与官方 farion1231 相同：`src-tauri/wix/per-user-main.wxs` 使用 `InstallScope=perUser` 且 `InstallPrivileges=limited`。这是有意的 per-user 安装，不会、也不能在安装时提升权限去改系统目录 ACL。

## 现象

安装或升级 MSI 时回滚，日志里常见：

- `Error 5` / `拒绝访问`
- 目标路径为 `D:\Config.Msi` 或 `%SystemDrive%\Config.Msi`

`Config.Msi` 是 Windows Installer 的回滚缓存目录。Error 5 来自该目录（或其父卷）的 ACL，不是 CC Switch 应用目录、也不是数据库 SCHEMA。

## 为什么 MSI 里修不了

limited 权限的 per-user MSI **不能**改写 `D:\Config.Msi` 这类系统缓存 ACL。把安装改成 per-machine / 要求管理员，会偏离官方 3.20.x 打包模型，也解决不了所有机器上已损坏的 Installer 缓存。

因此本 fork **不**为 Error 5 改 WiX、不发半成品 MSI、不 bump `SCHEMA_VERSION`。

## 推荐做法（Portable 优先）

1. 从 Releases 下载 `CC-Switch-v{版本号}-Windows-Portable.zip`
2. 解压到任意用户可写目录
3. 运行 `CC-Switch.exe`

绿色版不走 Windows Installer，因此不受 `Config.Msi` ACL 影响。配置与数据库仍在用户目录（或便携目录），与 SCHEMA 18 兼容。

若仍要使用 MSI：先在一台能提升权限的会话里检查/修复 `D:\Config.Msi`（或系统盘 `Config.Msi`）的 ACL，再重试官方同款 per-user 安装包。这是机器环境问题，不是应用打包缺陷。
