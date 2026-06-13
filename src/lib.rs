/// 通用工具模块
pub mod cli;
pub mod config;
pub mod datetime;
pub mod logging;

#[cfg(test)]
pub(crate) mod test_util {
    use std::sync::{Mutex, OnceLock};

    /// 全局 HOME 锁，串行化所有修改 HOME 的测试
    static HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    /// 获取 HOME 锁守卫
    pub(crate) fn acquire_home_lock() -> std::sync::MutexGuard<'static, ()> {
        HOME_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 在临时 HOME 目录下执行测试函数
    /// 使用全局锁确保 HOME 环境变量不会被并行测试竞态覆盖
    pub fn run_with_temp_home(f: impl FnOnce(&std::path::Path)) {
        let _guard = acquire_home_lock();
        let dir = tempfile::tempdir().unwrap();
        let orig_home = std::env::var("HOME").ok();
        // SAFETY: HOME_LOCK 确保无竞态
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        f(dir.path());
        match orig_home {
            Some(h) => unsafe {
                std::env::set_var("HOME", h);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
    }
}
