//! Shared, versioned offline license format. Only the issuer holds a signing key.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_CODE_BYTES: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct License {
    pub version: u32,
    pub product: String,
    pub machine: String,
    pub customer: String,
    pub issued_at: u64,
}

pub fn machine_code(identity: &str) -> String {
    let digest = Sha256::digest(format!(
        "LanPrint machine v1:{}",
        identity.trim().to_lowercase()
    ));
    format!("LP1-{}", hex::encode_upper(&digest[..16]))
}

pub fn normalize_machine(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_uppercase();
    let digits = value.strip_prefix("LP1-").ok_or("机器码应以 LP1- 开头。")?;
    if digits.len() != 32 || !digits.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("机器码格式错误，请完整复制注册页面上的机器码。".into());
    }
    Ok(value)
}

pub fn validate_claims(license: &License, machine: &str) -> Result<(), String> {
    if license.version != 1 || license.product != "lanprint" {
        return Err("此注册码不适用于当前产品或版本。".into());
    }
    if license.machine != normalize_machine(machine)? {
        return Err("注册码与本机不匹配，请使用本机机器码重新申请。".into());
    }
    let customer = license.customer.trim();
    if customer.is_empty()
        || customer.chars().count() > 80
        || customer.chars().any(char::is_control)
    {
        return Err("授权名称应为 1–80 字，且不能包含控制字符。".into());
    }
    Ok(())
}

pub fn verify_code(code: &str, machine: &str, public_key: &[u8; 32]) -> Result<License, String> {
    if code.len() > MAX_CODE_BYTES {
        return Err("注册码过长。".into());
    }
    // Email and chat software may insert line breaks into long codes.
    let compact: String = code.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let parts: Vec<_> = compact.split('.').collect();
    if parts.len() != 3 || parts[0] != "LP1" {
        return Err("注册码格式错误，请粘贴完整的 LP1. 注册码。".into());
    }
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| "注册码内容无效。")?;
    let signature = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| "注册码签名无效。")?;
    let signature = Signature::from_slice(&signature).map_err(|_| "注册码签名无效。")?;
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| "程序的授权公钥无效。")?;
    key.verify_strict(&payload, &signature)
        .map_err(|_| "注册码验证失败，注册码可能有误或已被修改。")?;
    let license: License = serde_json::from_slice(&payload).map_err(|_| "授权信息格式错误。")?;
    validate_claims(&license, machine)?;
    Ok(license)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    fn issue(key: &SigningKey, license: &License) -> String {
        let payload = serde_json::to_vec(license).unwrap();
        format!(
            "LP1.{}.{}",
            URL_SAFE_NO_PAD.encode(&payload),
            URL_SAFE_NO_PAD.encode(key.sign(&payload).to_bytes())
        )
    }
    #[test]
    fn signature_machine_and_claims_are_enforced() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let machine = machine_code("test-installation");
        let mut license = License {
            version: 1,
            product: "lanprint".into(),
            machine: machine.clone(),
            customer: "测试办公室".into(),
            issued_at: 1,
        };
        let code = issue(&key, &license);
        let public = key.verifying_key().to_bytes();
        assert_eq!(verify_code(&code, &machine, &public).unwrap(), license);
        assert!(verify_code(&format!(" \n{code}\r\n"), &machine, &public).is_ok());
        assert!(verify_code(&code, &machine_code("other"), &public).is_err());
        assert!(
            verify_code(
                &code,
                &machine,
                &SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes()
            )
            .is_err()
        );
        let mut parts: Vec<String> = code.split('.').map(str::to_string).collect();
        license.customer = "篡改".into();
        parts[1] = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&license).unwrap());
        assert!(verify_code(&parts.join("."), &machine, &public).is_err());
        license.product = "another-product".into();
        assert!(verify_code(&issue(&key, &license), &machine, &public).is_err());
        license.product = "lanprint".into();
        license.version = 2;
        assert!(verify_code(&issue(&key, &license), &machine, &public).is_err());
        for invalid in ["", "LP1.x.y", "LP1.x.y.z", &"x".repeat(MAX_CODE_BYTES + 1)] {
            assert!(verify_code(invalid, &machine, &public).is_err());
        }
    }
    #[test]
    fn machine_identity_is_stable_and_normalized() {
        assert_eq!(machine_code(" ABC "), machine_code("abc"));
        let code = machine_code("abc");
        assert_eq!(normalize_machine(&code.to_lowercase()).unwrap(), code);
        assert!(normalize_machine("LP1-invalid").is_err());
    }
}
