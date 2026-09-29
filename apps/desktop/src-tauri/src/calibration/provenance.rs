use crate::calibration::{validated_calibration_profile, CalibrationError, CalibrationProfileV1};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

const FINGERPRINT_VERSION: u8 = 1;
const FINGERPRINT_ALGORITHM: &str = "sha256";
const FINGERPRINT_DOMAIN_PREFIX: &[u8] = b"anchor:calibration-profile:v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationProfileFingerprintV1 {
    pub version: u8,
    pub algorithm: String,
    pub digest: String,
}

#[derive(Debug)]
pub enum FingerprintError {
    Calibration(CalibrationError),
    Serialization(String),
    Invalid(String),
}

impl fmt::Display for FingerprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Calibration(err) => write!(f, "{err}"),
            Self::Serialization(err) | Self::Invalid(err) => f.write_str(err),
        }
    }
}

impl std::error::Error for FingerprintError {}

impl From<CalibrationError> for FingerprintError {
    fn from(value: CalibrationError) -> Self {
        Self::Calibration(value)
    }
}

impl CalibrationProfileFingerprintV1 {
    pub fn for_profile(profile: &CalibrationProfileV1) -> Result<Self, FingerprintError> {
        let validated = validated_calibration_profile(profile)?;
        Self::for_canonical_validated_profile(&validated)
    }

    fn for_canonical_validated_profile(
        profile: &CalibrationProfileV1,
    ) -> Result<Self, FingerprintError> {
        let bytes = serde_json::to_vec(profile)
            .map_err(|err| FingerprintError::Serialization(format!("profile JSON: {err}")))?;
        let mut hasher = Sha256::new();
        hasher.update(FINGERPRINT_DOMAIN_PREFIX);
        hasher.update(bytes);
        let digest = hasher.finalize();
        Ok(Self {
            version: FINGERPRINT_VERSION,
            algorithm: FINGERPRINT_ALGORITHM.to_owned(),
            digest: hex_lower(&digest),
        })
    }

    pub fn validate(&self) -> Result<(), FingerprintError> {
        if self.version != FINGERPRINT_VERSION {
            return Err(FingerprintError::Invalid(format!(
                "unsupported fingerprint version: {}",
                self.version
            )));
        }
        if self.algorithm != FINGERPRINT_ALGORITHM {
            return Err(FingerprintError::Invalid(format!(
                "fingerprint algorithm must be {FINGERPRINT_ALGORITHM}"
            )));
        }
        if self.digest.len() != 64
            || !self
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(FingerprintError::Invalid(
                "fingerprint digest must be 64 lowercase hexadecimal characters".to_owned(),
            ));
        }
        Ok(())
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion_filtering::synthetic::synthetic_suite;

    fn profile() -> CalibrationProfileV1 {
        synthetic_suite().unwrap().remove(0).calibration_profile
    }

    #[test]
    fn fingerprint_is_stable_and_validates_format() {
        let profile = profile();
        let pretty = serde_json::to_string_pretty(&profile).unwrap();
        let compact = serde_json::to_string(&profile).unwrap();
        let pretty_profile: CalibrationProfileV1 = serde_json::from_str(&pretty).unwrap();
        let compact_profile: CalibrationProfileV1 = serde_json::from_str(&compact).unwrap();
        let a = CalibrationProfileFingerprintV1::for_profile(&pretty_profile).unwrap();
        let b = CalibrationProfileFingerprintV1::for_profile(&compact_profile).unwrap();
        assert_eq!(a, b);
        a.validate().unwrap();
    }

    #[test]
    fn fingerprint_changes_when_normative_field_changes() {
        let mut profile = profile();
        let a = CalibrationProfileFingerprintV1::for_profile(&profile).unwrap();
        profile.source_dataset = "other.ndjson".to_owned();
        let b = CalibrationProfileFingerprintV1::for_profile(&profile).unwrap();
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn invalid_fingerprint_format_is_rejected() {
        for digest in ["", "ABC", &"g".repeat(64)] {
            let fp = CalibrationProfileFingerprintV1 {
                version: 1,
                algorithm: "sha256".to_owned(),
                digest: digest.to_owned(),
            };
            assert!(fp.validate().is_err());
        }
        assert!(CalibrationProfileFingerprintV1 {
            version: 2,
            algorithm: "sha256".to_owned(),
            digest: "0".repeat(64),
        }
        .validate()
        .is_err());
    }
}
