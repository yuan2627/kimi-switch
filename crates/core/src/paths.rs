//! 统一路径解析。所有 Provider 元数据、审计日志、状态文件都从这里取。
//!
//! 遵循 XDG（Linux）/ Library/Application Support（macOS）/ AppData（Windows）。

use crate::error::{Error, Result};
use directories::ProjectDirs;
use std::path::PathBuf;

/// 从旧版本数据目录一次性迁移（老用户的账号库不丢）。
fn migrate_legacy_dirs(new_dirs: &ProjectDirs) {
    let Some(old) = ProjectDirs::from("dev", "kimi-switch", "kimi-switch") else {
        return;
    };
    for (old_dir, new_dir) in [
        (old.config_dir(), new_dirs.config_dir()),
        (old.data_dir(), new_dirs.data_dir()),
        (old.cache_dir(), new_dirs.cache_dir()),
    ] {
        if old_dir.exists() && !new_dir.exists() {
            // Windows 下 rename 要求目标父目录已存在，先补齐。
            if let Some(parent) = new_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::rename(old_dir, new_dir);
        }
    }
}

/// 项目维度的标准路径集合。
pub struct AppPaths {
    /// 配置：registry.toml、provider 元数据。
    pub config_dir: PathBuf,
    /// 数据：审计日志、备份快照。
    pub data_dir: PathBuf,
    /// 运行时状态：当前激活账号缓存、daemon pid 等。
    pub state_dir: PathBuf,
    /// 缓存：额度查询缓存等可丢弃数据。
    pub cache_dir: PathBuf,
}

impl AppPaths {
    /// 解析默认路径；目录不存在时会自动创建。
    ///
    /// 设置 `KIMI_SWITCH_HOME` 时，配置、数据、状态与缓存会全部收口到该绝对路径下。
    /// 该入口用于跨平台隔离运行与测试，避免 Windows 系统目录无法由 XDG 变量重定向。
    pub fn resolve() -> Result<Self> {
        let (config_dir, data_dir, cache_dir) = match std::env::var_os("KIMI_SWITCH_HOME") {
            Some(root) => {
                let root = PathBuf::from(root);
                if !root.is_absolute() {
                    return Err(Error::Config(
                        "KIMI_SWITCH_HOME must be an absolute path".into(),
                    ));
                }
                (root.join("config"), root.join("data"), root.join("cache"))
            }
            None => {
                let dirs = ProjectDirs::from("dev", "kimi-switch", "kimi-switch")
                    .ok_or_else(|| Error::Config("cannot resolve user directories".into()))?;
                migrate_legacy_dirs(&dirs);
                (
                    dirs.config_dir().to_path_buf(),
                    dirs.data_dir().to_path_buf(),
                    dirs.cache_dir().to_path_buf(),
                )
            }
        };
        // ProjectDirs 没有 state_dir 抽象，按平台约定挂在 data_dir 下。
        let state_dir = data_dir.join("state");

        for d in [&config_dir, &data_dir, &state_dir, &cache_dir] {
            std::fs::create_dir_all(d)?;
        }

        Ok(Self {
            config_dir,
            data_dir,
            state_dir,
            cache_dir,
        })
    }

    /// 账号注册表路径：`<config_dir>/registry.toml`。
    pub fn registry_file(&self) -> PathBuf {
        self.config_dir.join("registry.toml")
    }

    /// 数值调优配置文件路径：`<config_dir>/config.toml`。
    ///
    /// 文件可缺失：缺则使用 [`crate::defaults`] 中的编译期默认值。详见 [`crate::settings`]。
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// 明文凭证文件：`<data_dir>/credentials.json`（[`crate::store::FileStore`] 后端，`0600`）。
    /// 放 data 而非 config，避免被随 config 一起同步出去。
    pub fn credentials_file(&self) -> PathBuf {
        self.data_dir.join("credentials.json")
    }

    /// 审计日志：`<data_dir>/audit.log`。
    pub fn audit_log(&self) -> PathBuf {
        self.data_dir.join("audit.log")
    }

    /// 切换前快照根目录：`<state_dir>/snapshots/`。
    pub fn snapshots_dir(&self) -> PathBuf {
        self.state_dir.join("snapshots")
    }

    /// kimi-switchd 守护进程 PID 文件:`<state_dir>/kimi-switchd.pid`。
    /// 通过 fs2 文件锁标识唯一存活实例;退出后保留 PID 仅作信息参考。
    pub fn daemon_pid_file(&self) -> PathBuf {
        self.state_dir.join("kimi-switchd.pid")
    }

    /// kimi-switchd 守护进程日志文件:`<data_dir>/kimi-switchd.log`。
    /// 用 append 模式打开,后续可由 logrotate 切割。
    pub fn daemon_log_file(&self) -> PathBuf {
        self.data_dir.join("kimi-switchd.log")
    }

    /// quota 查询结果缓存：`<cache_dir>/quota_cache.json`。
    pub fn quota_cache_file(&self) -> PathBuf {
        self.cache_dir.join("quota_cache.json")
    }

    /// 用户显式删除过的账号墓碑：`<config_dir>/removed.json`。
    /// 默认入口自动导入会跳过这些 id，直到 `kimi-switch login` 再导入。
    pub fn removed_file(&self) -> PathBuf {
        self.config_dir.join("removed.json")
    }
}
