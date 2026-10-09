//! API Key 通过系统凭据库保存（Windows 凭据管理器 / macOS Keychain），
//! 不进入配置文件、历史、日志或导出。

fn service() -> String {
    #[cfg(debug_assertions)]
    if let Some(path) = crate::config::qa_data_dir() {
        return format!("config-reviewer-qa-{}", path.display());
    }
    "config-reviewer".into()
}
const USER: &str = "gateway-api-key";

pub fn save_api_key(key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(&service(), USER).map_err(|e| format!("凭据库不可用：{e}"))?;
    if key.trim().is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("清除 API Key 失败：{e}")),
        };
    }
    entry
        .set_password(key.trim())
        .map_err(|e| format!("保存 API Key 失败：{e}"))
}

pub fn load_api_key() -> String {
    keyring::Entry::new(&service(), USER)
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .unwrap_or_default()
}

pub fn has_api_key() -> bool {
    !load_api_key().is_empty()
}
