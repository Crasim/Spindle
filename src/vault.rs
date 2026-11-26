use anyhow::{Context, Result, anyhow, ensure};
use ring::{
    aead, pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use std::num::NonZeroU32;

pub fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow!("System random source unavailable"))?;
    Ok(bytes)
}

pub fn derive(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    let mut key = [0; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(600_000).unwrap(),
        salt,
        password.as_bytes(),
        &mut key,
    );
    key
}

fn cipher(key: &[u8; 32]) -> Result<aead::LessSafeKey> {
    Ok(aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, key)
            .map_err(|_| anyhow!("Invalid encryption key"))?,
    ))
}

pub fn seal(key: &[u8; 32], aad: &[u8], mut data: Vec<u8>) -> Result<Vec<u8>> {
    let nonce = random::<12>()?;
    cipher(key)?
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut data,
        )
        .map_err(|_| anyhow!("Encryption failed"))?;
    let mut sealed = nonce.to_vec();
    sealed.extend(data);
    Ok(sealed)
}

pub fn open(key: &[u8; 32], aad: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= 28, "Incomplete encrypted record");
    let mut body = data[12..].to_vec();
    let nonce = aead::Nonce::try_assume_unique_for_key(&data[..12])
        .map_err(|_| anyhow!("Invalid nonce"))?;
    Ok(cipher(key)?
        .open_in_place(nonce, aead::Aad::from(aad), &mut body)
        .map_err(|_| anyhow!("Wrong passphrase or damaged encrypted record"))?
        .to_vec())
}

#[cfg(windows)]
pub fn local(data: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows::{
        Security::Cryptography::{CryptographicBuffer, DataProtection::DataProtectionProvider},
        core::{Array, HSTRING},
    };
    let input = CryptographicBuffer::CreateFromByteArray(data)?;
    let output = if encrypt {
        DataProtectionProvider::CreateOverloadExplicit(&HSTRING::from("LOCAL=user"))?
            .ProtectAsync(&input)?
            .get()?
    } else {
        DataProtectionProvider::new()?
            .UnprotectAsync(&input)?
            .get()
            .context("Unlock this session with its original Windows account")?
    };
    let mut result = Array::<u8>::new();
    CryptographicBuffer::CopyToByteArray(&output, &mut result)?;
    Ok(result.to_vec())
}

#[cfg(not(windows))]
pub fn local(data: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    let password = std::env::var("SPINDLE_SESSION_PASSPHRASE")
        .context("Set SPINDLE_SESSION_PASSPHRASE to protect this device's session")?;
    ensure!(
        password.chars().count() >= 12,
        "Use at least 12 characters for the session passphrase"
    );
    if encrypt {
        let salt = random::<16>()?;
        let mut result = salt.to_vec();
        result.extend(seal(
            &derive(&password, &salt),
            b"Spindle local session v1",
            data.to_vec(),
        )?);
        Ok(result)
    } else {
        let salt = data.get(..16).context("Invalid session")?.try_into()?;
        open(
            &derive(&password, &salt),
            b"Spindle local session v1",
            &data[16..],
        )
    }
}
