# kimi-switch · Agent 快速规则

Kimi Code 多账号管理工具（Rust workspace）：GUI（`kimi-switch.exe`）+ CLI（`kimi-switch-cli.exe`）。

## 目录速记

```text
crates/core/             数据模型、Provider trait、凭证仓库、路径、额度缓存
crates/cli/              kimi-switch-cli 命令行
crates/gui/              kimi-switch 图形界面（eframe/egui）
crates/providers/common/ 文件型 OAuth 账号切换共享引擎（FileBlobProvider/FileBlobRuntime）
crates/providers/kimi/   Kimi 适配器（设备码授权、token 刷新协调、额度查询）
```

## 规则

1. **改完必须验证**：`cargo check --workspace && cargo test --workspace`，通过后再 `cargo build --release`。
2. **凭证安全是最高优先级**：token 只能写用户本机私有文件；网络请求只允许发往 Kimi 官方域名（`auth.kimi.com` / `api.kimi.com`）；禁止引入遥测。
3. **手动切换永远可用**：`swap` 不依赖额度查询和网络，原子写入 + 快照回滚的逻辑不得破坏。
4. **token 刷新必须与官方客户端协调**：只能复用官方锁协议，不得自创并行刷新机制；refresh token 是一次性轮换，被拒绝后记录指纹并停止重试。
5. 代码注释、doc comment 用中文；用户可见输出、错误文本用简洁中文（GUI）/英文（CLI 内部标识）。
6. 不做 git 提交、推送、打 tag，除非用户明确要求。

## 常用命令

```bash
cargo check --workspace
cargo test --workspace
cargo build --release   # 产物：target/release/kimi-switch.exe / kimi-switch-cli.exe
```
