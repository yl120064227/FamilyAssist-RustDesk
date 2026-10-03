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

pub const OPTION_FAMILY_GUARD: &str = "family-guard";

/// 确认等待超时（秒）的本地配置键——超时未确认按拒绝处理；开发/测试可调小
pub const OPTION_CONSENT_TIMEOUT: &str = "family-guard-consent-timeout";
const DEFAULT_CONSENT_TIMEOUT_SECS: u64 = 60;

/// 家庭守卫是否启用（默认启用；本地显式置 "N" 关闭，仅供开发）
pub fn enabled() -> bool {
    crate::get_local_option(OPTION_FAMILY_GUARD) != "N"
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
