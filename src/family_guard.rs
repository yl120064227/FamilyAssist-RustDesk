//! FamilyAssist 家庭守卫 —— G1 授权内核 WP1：配置层锁定
//!
//! 被控端"本地同意"不可绕过：
//! - `approve-mode` 锁 `click`：上游默认 `Both`，持有有效密码（含一次性密码）的
//!   连接会跳过本机确认直入（connection.rs 登录决策树 L2676-2693），此路径必须封死
//! - 永久密码禁用（三处修改入口失效 + 存量清空）
//! - 信任设备（2FA 旁路面）禁用 + 存量清空
//! - `allow-hide-cm` / `access-mode=full` 等旁路面锁定
//!
//! 实现复用上游 custom-client 的 `OVERWRITE_SETTINGS` / `BUILTIN_SETTINGS` 机制：
//! 读取优先级最高（`Config::get_option` 先查 OVERWRITE），写入被 `is_option_can_save`
//! 拒绝，篡改配置文件或改选项都无法生效。决策树侧另有读时护栏双保险（见
//! connection.rs 中 `family_guard::enabled()` 调用点）。
//!
//! 开关：LocalConfig `family-guard`（RustDesk_local.toml，不参与任何同步），
//! 缺省启用；显式置 `N` 仅供开发调试，长辈设备保持缺省。

use hbb_common::{
    config::{keys, Config, BUILTIN_SETTINGS, OVERWRITE_SETTINGS},
    log,
};
use std::io::Write as _;

pub const OPTION_FAMILY_GUARD: &str = "family-guard";

/// 确认等待超时（秒）的本地配置键——超时未确认按拒绝处理；开发/测试可调小
pub const OPTION_CONSENT_TIMEOUT: &str = "family-guard-consent-timeout";
const DEFAULT_CONSENT_TIMEOUT_SECS: u64 = 60;

/// 家庭守卫是否启用（默认启用；本地显式置 "N" 关闭，仅供开发）
pub fn enabled() -> bool {
    crate::get_local_option(OPTION_FAMILY_GUARD) != "N"
}

// ---------- 本地脱敏审计（WP7） ----------
// 任务包 02 数据流约束：只记录 session 标识（脱敏）、端类型、状态、时长、错误码；
// 禁止凭证（密码/密钥）与输入内容；文件自动截断。
// 自建 OSS hbbs 无 API server（上游 post_conn_audit 空转），审计落本地文件。

const AUDIT_FILE: &str = "family-audit.log";
const AUDIT_MAX_BYTES: u64 = 512 * 1024;

fn audit_path() -> Option<std::path::PathBuf> {
    Some(Config::get_home().join(AUDIT_FILE))
}

/// 追加一条审计事件（JSON 行）。绝不写入凭证/输入内容——调用方约束见 WP7 文档。
pub fn audit_event(event: &str, conn_id: i32, detail: &str) {
    if !enabled() {
        return;
    }
    let Some(path) = audit_path() else {
        return;
    };
    // 简单轮转：超限则截断保留后半，避免审计文件无限增长
    if let Ok(md) = std::fs::metadata(&path) {
        if md.len() > AUDIT_MAX_BYTES {
            match std::fs::read(&path) {
                Ok(data) => {
                    let keep = &data[data.len() / 2..];
                    // 找到下一行边界，避免半行 JSON
                    let start = keep.iter().position(|&b| b == b'\n').map(|p| p + 1).unwrap_or(0);
                    if let Err(e) = std::fs::write(&path, &keep[start..]) {
                        log::warn!("family guard audit rotate failed: {}", e);
                        return;
                    }
                }
                Err(e) => {
                    log::warn!("family guard audit read failed: {}", e);
                    return;
                }
            }
        }
    }
    let line = audit_line(event, conn_id, detail, chrono_like_ts());
    let mut file = match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("family guard audit open failed: {}", e);
            return;
        }
    };
    if let Err(e) = file.write_all(line.as_bytes()) {
        log::warn!("family guard audit write failed: {}", e);
    }
}

// Use the JSON serializer for both string fields: peer-controlled text and close
// reasons may contain newlines, quotes or control characters.
fn audit_line(event: &str, conn_id: i32, detail: &str, timestamp: u64) -> String {
    let mut line = serde_json::json!({
        "ts": timestamp,
        "event": event,
        "conn_id": conn_id,
        "detail": detail,
    })
    .to_string();
    line.push('\n');
    line
}

fn chrono_like_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 确认等待超时秒数（默认 60；LocalConfig 可覆盖，非法值回退默认）
pub fn consent_timeout_secs() -> u64 {
    crate::get_local_option(OPTION_CONSENT_TIMEOUT)
        .parse()
        .unwrap_or(DEFAULT_CONSENT_TIMEOUT_SECS)
}

/// 在 `load_custom_client()` 之后调用：先清危险存量，再注入锁定。
/// 幂等，可安全重复调用。
pub fn apply() {
    if !enabled() {
        log::info!("family guard disabled by local option");
        return;
    }
    // 必须先清存量再锁：锁定后这些写入会被 is_disable_change_permanent_password 拒绝
    if Config::set_permanent_password("") {
        log::info!("family guard: cleared existing permanent password");
    }
    Config::clear_trusted_devices();

    let mut overwrite = OVERWRITE_SETTINGS.write().unwrap();
    // 唯一放行入口 = 本机点击确认；密码（含一次性密码）不再构成免确认直入
    overwrite.insert(
        keys::OPTION_APPROVE_MODE.to_owned(),
        "click".to_owned(),
    );
    // 一次性密码保留（作为确认弹窗的身份辅助显示）；永久密码不可用
    overwrite.insert(
        keys::OPTION_VERIFICATION_METHOD.to_owned(),
        "use-temporary-password".to_owned(),
    );
    // CM 确认窗口不得隐藏（password 模式才可能隐藏，已同时被 approve-mode 封死）
    overwrite.insert("allow-hide-cm".to_owned(), "N".to_owned());
    // 信任设备（2FA 旁路面）禁用
    overwrite.insert(
        keys::OPTION_ENABLE_TRUSTED_DEVICES.to_owned(),
        "N".to_owned(),
    );
    // access-mode 锁默认：禁 "full" 一键放权全部能力
    overwrite.insert(keys::OPTION_ACCESS_MODE.to_owned(), String::new());
    drop(overwrite);

    let mut builtin = BUILTIN_SETTINGS.write().unwrap();
    // 永久密码修改入口全部失效（ipc / ui_interface / core_main 均查此开关）
    builtin.insert(
        keys::OPTION_DISABLE_CHANGE_PERMANENT_PASSWORD.to_owned(),
        "Y".to_owned(),
    );
    drop(builtin);

    log::info!(
        "family guard applied: approve-mode=click, permanent-password & trusted-devices disabled"
    );
}

#[cfg(test)]
mod tests {
    use super::audit_line;

    #[test]
    fn audit_strings_round_trip_without_extra_lines() {
        for text in [
            "ordinary text",
            "设备名称与关闭原因 🛑",
            "quote\" slash\\ newline\nreturn\rtab\t nul\0",
            "\n{\"event\":\"authorized\"}\n",
        ] {
            let line = audit_line(text, -7, text, 123);
            assert_eq!(line.bytes().filter(|&b| b == b'\n').count(), 1);
            assert!(line.ends_with('\n'));
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(value["event"], text);
            assert_eq!(value["detail"], text);
            assert_eq!(value["conn_id"], -7);
            assert_eq!(value["ts"], 123);
        }
    }

    #[test]
    fn audit_escapes_every_ascii_control_character() {
        let text: String = (0u8..32).map(char::from).collect();
        let line = audit_line(&text, 0, &text, 0);
        assert!(line[..line.len() - 1].bytes().all(|b| b >= 32));
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["event"], text);
        assert_eq!(value["detail"], text);
    }
}
