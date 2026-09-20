//! API Key 通过系统凭据库保存（Windows 凭据管理器 / macOS Keychain），
//! 不进入配置文件、历史、日志或导出。

const SERVICE: &str = "config-reviewer";
const USER: &str = "gateway-api-key";

pub fn save_api_key(key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, USER).map_err(|e| format!("凭据库不可用：{e}"))?;
    if key.trim().is_empty() {
        let _ = entry.delete_credential();
        return Ok(());
    }
    entry
        .set_password(key.trim())
        .map_err(|e| format!("保存 API Key 失败：{e}"))
}

pub fn load_api_key() -> String {
    keyring::Entry::new(SERVICE, USER)
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .unwrap_or_default()
}

pub fn has_api_key() -> bool {
    !load_api_key().is_empty()
}
