use crate::{
    SegmentFormat,
    error::{IoriError, IoriResult},
};
use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use iori_hls::KeyMethod;
use reqwest::Client;
use std::{
    collections::HashMap,
    ffi::OsString,
    fs::File,
    io::{Cursor, Read, Write},
    path::PathBuf,
};
use tokio::process::Command;

#[derive(Debug)]
pub enum IoriKey {
    Aes128 {
        key: [u8; 16],
        iv: [u8; 16],
        /// The IV was derived from the media sequence instead of the key line.
        implicit_iv: bool,
    },
    ClearKey {
        keys: HashMap<String, String>,
    },
    SampleAes {
        key: [u8; 16],
        iv: [u8; 16],
        /// The IV was derived from the media sequence instead of the key line.
        implicit_iv: bool,
        keys: HashMap<String, String>,
    },
}

impl IoriKey {
    pub fn clear_key(key: &str) -> IoriResult<Self> {
        let (kid, key) = key
            .split_once(':')
            .ok_or_else(|| IoriError::InvalidHexKey(key.to_string()))?;

        let mut keys = HashMap::new();
        keys.insert(kid.to_string(), key.to_string());
        Ok(Self::ClearKey { keys })
    }

    pub async fn from_key(
        client: &Client,
        key: &iori_hls::Key,
        playlist_url: &reqwest::Url,
        media_sequence: u64,
        manual_key: Option<String>,
    ) -> IoriResult<Option<Self>> {
        Ok(match &key.method {
            KeyMethod::None => None,
            KeyMethod::AES128 => {
                let key_bytes = if let Some(key) = manual_key {
                    hex::decode(key)?
                } else {
                    client
                        .get(
                            playlist_url
                                .join(&key.uri.clone().expect("URI field in key must exist"))?,
                        )
                        .send()
                        .await?
                        .bytes()
                        .await?
                        .to_vec()
                };
                let explicit_iv = parse_media_iv(key.iv.as_deref());
                Some(Self::Aes128 {
                    key: key_bytes.try_into().map_err(IoriError::InvalidBinaryKey)?,
                    iv: explicit_iv.unwrap_or_else(|| (media_sequence as u128).to_be_bytes()),
                    implicit_iv: explicit_iv.is_none(),
                })
            }
            KeyMethod::SampleAES => {
                let Some(manual_key) = manual_key else {
                    return Err(IoriError::DecryptionKeyRequired);
                };

                // Support two formats:
                // 1. <kid>:<key>;<kid>:<key>;...
                // 2. <key>
                let mut keys = HashMap::new();
                for pair in manual_key.split(';') {
                    match pair.split_once(':') {
                        Some((kid, key)) if is_valid_kid_key_pair(kid, key) => {
                            keys.insert(kid.to_string(), key.to_string());
                        }
                        _ => tracing::warn!("Ignored key-only format: {}", pair),
                    }
                }

                let raw_key = if keys.is_empty() {
                    manual_key
                } else {
                    keys.iter().next().unwrap().1.clone()
                };
                let key_bytes =
                    hex::decode(raw_key).map_err(|_| IoriError::DecryptionKeyRequired)?;

                let explicit_iv = parse_media_iv(key.iv.as_deref());
                Some(Self::SampleAes {
                    key: key_bytes.try_into().map_err(IoriError::InvalidBinaryKey)?,
                    iv: explicit_iv.unwrap_or_else(|| (media_sequence as u128).to_be_bytes()),
                    implicit_iv: explicit_iv.is_none(),
                    keys,
                })
            }
            name @ (KeyMethod::SampleAesCenc | KeyMethod::SampleAESCTR) => {
                tracing::debug!("Encryption method {name} detected. Using manual key.");

                // <kid>:<key>;<kid>:<key>;...
                let Some(manual_key) = manual_key else {
                    return Err(IoriError::DecryptionKeyRequired);
                };
                let mut keys = HashMap::new();
                for pair in manual_key.split(';') {
                    match pair.split_once(':') {
                        Some((kid, key)) if is_valid_kid_key_pair(kid, key) => {
                            keys.insert(kid.to_string(), key.to_string());
                        }
                        _ => tracing::warn!("Ignored invalid key format: {}", pair),
                    }
                }
                if keys.is_empty() {
                    return Err(IoriError::InvalidHexKey(manual_key));
                }

                Some(Self::ClearKey { keys })
            }
            KeyMethod::Other(name) => unimplemented!("Unknown key method: {name}"),
        })
    }

    /// Rebuild the key for a segment whose media sequence differs from the one
    /// the key was loaded for.
    ///
    /// AES keys without an explicit IV derive it from the media sequence, so a
    /// segment outside the playlist window needs its own copy. Clear keys do
    /// not depend on the sequence and return `None`; the caller can reuse the
    /// original key instead.
    pub fn with_media_sequence(&self, media_sequence: u64) -> Option<Self> {
        fn rederive(iv: &[u8; 16], implicit_iv: bool, media_sequence: u64) -> [u8; 16] {
            if implicit_iv {
                (media_sequence as u128).to_be_bytes()
            } else {
                *iv
            }
        }

        match self {
            Self::Aes128 {
                key,
                iv,
                implicit_iv,
            } => Some(Self::Aes128 {
                key: *key,
                iv: rederive(iv, *implicit_iv, media_sequence),
                implicit_iv: *implicit_iv,
            }),
            Self::SampleAes {
                key,
                iv,
                implicit_iv,
                keys,
            } => Some(Self::SampleAes {
                key: *key,
                iv: rederive(iv, *implicit_iv, media_sequence),
                implicit_iv: *implicit_iv,
                keys: keys.clone(),
            }),
            Self::ClearKey { .. } => None,
        }
    }

    pub fn to_decryptor(
        &self,
        segment_format: SegmentFormat,
        shaka_packager_command: Option<PathBuf>,
    ) -> IoriDecryptor {
        match self {
            IoriKey::Aes128 { key, iv, .. } => {
                IoriDecryptor::Aes128(Box::new(cbc::Decryptor::<aes::Aes128>::new(
                    key.into(),
                    iv.into(),
                )))
            }
            IoriKey::ClearKey { keys } => {
                if let Some(shaka_packager) = shaka_packager_command {
                    IoriDecryptor::ShakaPackager {
                        command: shaka_packager,
                        keys: keys.clone(),
                    }
                } else {
                    IoriDecryptor::Mp4Decrypt { keys: keys.clone() }
                }
            }
            IoriKey::SampleAes { key, iv, keys, .. } => match segment_format {
                SegmentFormat::Mpeg2TS | SegmentFormat::Aac => {
                    IoriDecryptor::SampleAes { key: *key, iv: *iv }
                }
                _ => {
                    if let Some(shaka_packager) = shaka_packager_command {
                        IoriDecryptor::ShakaPackager {
                            command: shaka_packager,
                            keys: keys.clone(),
                        }
                    } else {
                        IoriDecryptor::Mp4Decrypt { keys: keys.clone() }
                    }
                }
            },
        }
    }
}

pub enum IoriDecryptor {
    Aes128(Box<cbc::Decryptor<aes::Aes128>>),
    Mp4Decrypt {
        keys: HashMap<String, String>,
    },
    ShakaPackager {
        command: PathBuf,
        keys: HashMap<String, String>,
    },
    SampleAes {
        key: [u8; 16],
        iv: [u8; 16],
    },
}

impl IoriDecryptor {
    pub async fn decrypt(self, data: &[u8]) -> IoriResult<Vec<u8>> {
        Ok(match self {
            IoriDecryptor::Aes128(decryptor) => {
                tokio::task::block_in_place(|| decryptor.decrypt_padded_vec_mut::<Pkcs7>(data))?
            }
            IoriDecryptor::Mp4Decrypt { keys } => {
                tokio::task::block_in_place(|| mp4decrypt::mp4decrypt(data, &keys, None))?
            }
            IoriDecryptor::ShakaPackager { command, keys } => {
                let temp_dir = tempfile::tempdir()?;
                let rand_suffix = rand::random::<u64>();
                let temp_input_file = temp_dir.path().join(format!("input_{rand_suffix}.mp4"));
                let temp_output_file = temp_dir.path().join(format!("output_{rand_suffix}.mp4"));

                {
                    let mut input = File::create(&temp_input_file)?;
                    input.write_all(data)?;
                    input.flush()?;
                }

                let mut command = Command::new(command);
                command
                    .arg("--quiet")
                    .arg("--enable_raw_key_decryption")
                    .arg({
                        let mut str = OsString::new();
                        str.push("input=");
                        str.push(temp_input_file.as_os_str());
                        str.push(",stream=0,output=");
                        str.push(temp_output_file.as_os_str());
                        str
                    });

                if !keys.is_empty() {
                    use std::fmt::Write;
                    let mut arg = String::new();
                    for (index, (kid, key)) in keys.iter().enumerate() {
                        if index != 0 {
                            arg.push(',');
                        }
                        write!(arg, "label={}:key_id={kid}:key={key}", index + 1).unwrap();
                    }

                    command.arg("--keys").arg(arg);
                }

                command.spawn()?.wait().await?;

                let mut file = File::open(temp_output_file)?;
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;
                data
            }
            IoriDecryptor::SampleAes { key, iv } => {
                let mut reader = Cursor::new(data);
                let mut writer = Vec::new();
                tokio::task::block_in_place(|| {
                    iori_ssa::decrypt(&mut reader, &mut writer, key, iv).map(|_| writer)
                })?
            }
        })
    }
}

fn is_valid_kid_key_pair(kid: &str, key: &str) -> bool {
    kid.len() == 32
        && key.len() == 32
        && kid.chars().all(|c| c.is_ascii_hexdigit())
        && key.chars().all(|c| c.is_ascii_hexdigit())
}

fn parse_media_iv(iv: Option<&str>) -> Option<[u8; 16]> {
    let iv = iv?;
    let iv = iv.strip_prefix("0x").unwrap_or(iv);
    // RFC 8216 defines the IV as a 128-bit hex number.
    if iv.len() != 32 {
        return None;
    }
    Some(u128::from_str_radix(iv, 16).ok()?.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iori_hls::{Key, KeyFormat};

    fn aes_key(iv: Option<&str>) -> Key {
        Key {
            method: KeyMethod::AES128,
            uri: Some("key.bin".to_string()),
            iv: iv.map(str::to_string),
            key_format: KeyFormat::Identity,
            key_format_versions: None,
        }
    }

    async fn load_key(iv: Option<&str>, media_sequence: u64) -> IoriResult<IoriKey> {
        let client = Client::new();
        let url = reqwest::Url::parse("https://example.com/playlist.m3u8")?;
        Ok(IoriKey::from_key(
            &client,
            &aes_key(iv),
            &url,
            media_sequence,
            Some("0".repeat(32)),
        )
        .await?
        .expect("AES key expected"))
    }

    fn iv_of(key: &IoriKey) -> ([u8; 16], bool) {
        match key {
            IoriKey::Aes128 {
                iv, implicit_iv, ..
            } => (*iv, *implicit_iv),
            _ => panic!("expected an AES-128 key"),
        }
    }

    #[tokio::test]
    async fn implicit_iv_is_recomputed_for_another_media_sequence() -> anyhow::Result<()> {
        let key = load_key(None, 7).await?;
        assert_eq!(iv_of(&key), ((7u128).to_be_bytes(), true));

        let older = key.with_media_sequence(3).expect("AES key can be rebuilt");
        assert_eq!(iv_of(&older), ((3u128).to_be_bytes(), true));
        // Rebuilding a segment key must not disturb the key it came from.
        assert_eq!(iv_of(&key), ((7u128).to_be_bytes(), true));

        Ok(())
    }

    #[tokio::test]
    async fn explicit_iv_survives_another_media_sequence() -> anyhow::Result<()> {
        let explicit = "0x000102030405060708090a0b0c0d0e0f";
        let key = load_key(Some(explicit), 7).await?;
        let expected = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        assert_eq!(iv_of(&key), (expected, false));

        let older = key.with_media_sequence(3).expect("AES key can be rebuilt");
        assert_eq!(iv_of(&older), (expected, false));

        Ok(())
    }

    #[tokio::test]
    async fn clear_keys_are_reused_without_a_sequence() -> anyhow::Result<()> {
        let kid = "0".repeat(32);
        let key = IoriKey::clear_key(&format!("{kid}:{}", "1".repeat(32)))?;

        assert!(key.with_media_sequence(3).is_none());

        Ok(())
    }
}
