use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_DATAGRAM_BYTES: usize = 1024;
pub const MAX_SESSION_ELAPSED_US: u64 = 9_007_199_254_740_991;

const SESSION_ID_MIN_LEN: usize = 1;
const SESSION_ID_MAX_LEN: usize = 64;

const LINEAR_ACCELERATION_MIN_MPS2: f64 = -200.0;
const LINEAR_ACCELERATION_MAX_MPS2: f64 = 200.0;
const GRAVITY_MIN_MPS2: f64 = -20.0;
const GRAVITY_MAX_MPS2: f64 = 20.0;
const ANGULAR_VELOCITY_MIN_RAD_S: f64 = -100.0;
const ANGULAR_VELOCITY_MAX_RAD_S: f64 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolVersion;

impl ProtocolVersion {
    pub const V1: Self = Self;
}

impl TryFrom<u8> for ProtocolVersion {
    type Error = ProtocolValidationError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value == PROTOCOL_VERSION {
            Ok(Self::V1)
        } else {
            Err(ProtocolValidationError::UnsupportedProtocolVersion(value))
        }
    }
}

impl From<ProtocolVersion> for u8 {
    fn from(_: ProtocolVersion) -> Self {
        PROTOCOL_VERSION
    }
}

impl Serialize for ProtocolVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(PROTOCOL_VERSION)
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        ProtocolVersion::try_from(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PacketKind {
    MotionSample,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MotionSampleV1 {
    pub protocol_version: ProtocolVersion,
    pub kind: PacketKind,
    pub session_id: String,
    pub sequence: u32,
    pub session_elapsed_us: u64,
    pub linear_acceleration_mps2: Vector3,
    pub gravity_mps2: Vector3,
    pub angular_velocity_rad_s: Vector3,
}

pub fn parse_validated_datagram(bytes: &[u8]) -> Result<MotionSampleV1, ProtocolParseError> {
    if bytes.len() > MAX_DATAGRAM_BYTES {
        return Err(ProtocolParseError::DatagramTooLarge {
            actual: bytes.len(),
            max: MAX_DATAGRAM_BYTES,
        });
    }

    let sample = match serde_json::from_slice::<MotionSampleV1>(bytes) {
        Ok(sample) => sample,
        Err(err) if err.is_syntax() || err.is_eof() => {
            return Err(ProtocolParseError::MalformedJson)
        }
        Err(_) => return Err(ProtocolParseError::InvalidStructure),
    };

    sample
        .validate()
        .map_err(ProtocolParseError::InvalidSemantics)?;

    Ok(sample)
}

impl MotionSampleV1 {
    pub fn validate(&self) -> Result<(), ProtocolValidationError> {
        let session_id_len = self.session_id.chars().count();
        if !(SESSION_ID_MIN_LEN..=SESSION_ID_MAX_LEN).contains(&session_id_len) {
            return Err(ProtocolValidationError::SessionIdLength {
                actual: session_id_len,
            });
        }

        if self.session_elapsed_us > MAX_SESSION_ELAPSED_US {
            return Err(ProtocolValidationError::SessionElapsedUsOutOfRange {
                actual: self.session_elapsed_us,
            });
        }

        self.linear_acceleration_mps2.validate_range(
            "linearAccelerationMps2",
            LINEAR_ACCELERATION_MIN_MPS2,
            LINEAR_ACCELERATION_MAX_MPS2,
        )?;
        self.gravity_mps2
            .validate_range("gravityMps2", GRAVITY_MIN_MPS2, GRAVITY_MAX_MPS2)?;
        self.angular_velocity_rad_s.validate_range(
            "angularVelocityRadS",
            ANGULAR_VELOCITY_MIN_RAD_S,
            ANGULAR_VELOCITY_MAX_RAD_S,
        )?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Vector3 {
    /// Axis in the frame assumed by the current mounting convention: X+ = right.
    pub x: f64,
    /// Axis in the frame assumed by the current mounting convention: Y+ = forward.
    pub y: f64,
    /// Axis in the frame assumed by the current mounting convention: Z+ = up.
    pub z: f64,
}

impl Vector3 {
    fn validate_range(
        &self,
        field_name: &'static str,
        min: f64,
        max: f64,
    ) -> Result<(), ProtocolValidationError> {
        validate_component(field_name, "x", self.x, min, max)?;
        validate_component(field_name, "y", self.y, min, max)?;
        validate_component(field_name, "z", self.z, min, max)?;
        Ok(())
    }
}

fn validate_component(
    field_name: &'static str,
    axis: &'static str,
    value: f64,
    min: f64,
    max: f64,
) -> Result<(), ProtocolValidationError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(ProtocolValidationError::VectorComponentOutOfRange {
            field_name,
            axis,
            min,
            max,
            actual: value,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolValidationError {
    UnsupportedProtocolVersion(u8),
    SessionIdLength {
        actual: usize,
    },
    SessionElapsedUsOutOfRange {
        actual: u64,
    },
    VectorComponentOutOfRange {
        field_name: &'static str,
        axis: &'static str,
        min: f64,
        max: f64,
        actual: f64,
    },
}

impl fmt::Display for ProtocolValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion(version) => {
                write!(f, "unsupported protocolVersion: {}", version)
            }
            Self::SessionIdLength { actual } => write!(
                f,
                "sessionId length must be between {} and {} characters, got {}",
                SESSION_ID_MIN_LEN, SESSION_ID_MAX_LEN, actual
            ),
            Self::SessionElapsedUsOutOfRange { actual } => write!(
                f,
                "sessionElapsedUs must be between 0 and {}, got {}",
                MAX_SESSION_ELAPSED_US, actual
            ),
            Self::VectorComponentOutOfRange {
                field_name,
                axis,
                min,
                max,
                actual,
            } => write!(
                f,
                "{}.{} must be between {} and {}, got {}",
                field_name, axis, min, max, actual
            ),
        }
    }
}

impl Error for ProtocolValidationError {}

#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolParseError {
    DatagramTooLarge { actual: usize, max: usize },
    MalformedJson,
    InvalidStructure,
    InvalidSemantics(ProtocolValidationError),
}

impl fmt::Display for ProtocolParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatagramTooLarge { actual, max } => {
                write!(f, "datagram too large: {} bytes exceeds {}", actual, max)
            }
            Self::MalformedJson => write!(f, "malformed JSON datagram"),
            Self::InvalidStructure => write!(f, "invalid datagram structure"),
            Self::InvalidSemantics(err) => write!(f, "invalid datagram semantics: {}", err),
        }
    }
}

impl Error for ProtocolParseError {}

#[cfg(test)]
mod tests {
    use super::{
        parse_validated_datagram, MotionSampleV1, PacketKind, ProtocolParseError,
        ProtocolValidationError, MAX_DATAGRAM_BYTES, PROTOCOL_VERSION,
    };
    use serde_json::{json, Value};

    const VALID_FIXTURE: &str =
        include_str!("../../../../packages/protocol/fixtures/valid/motion-sample.json");
    const INVALID_UNSUPPORTED_VERSION: &str =
        include_str!("../../../../packages/protocol/fixtures/invalid/unsupported-version.json");
    const INVALID_MISSING_FIELD: &str =
        include_str!("../../../../packages/protocol/fixtures/invalid/missing-field.json");
    const INVALID_OUT_OF_RANGE: &str =
        include_str!("../../../../packages/protocol/fixtures/invalid/out-of-range.json");
    const INVALID_UNKNOWN_PROPERTY: &str =
        include_str!("../../../../packages/protocol/fixtures/invalid/unknown-property.json");

    #[test]
    fn valid_fixture_deserializes_and_validates() {
        let sample: MotionSampleV1 = serde_json::from_str(VALID_FIXTURE).expect("valid fixture");

        sample.validate().expect("valid fixture should validate");
        assert_eq!(u8::from(sample.protocol_version), PROTOCOL_VERSION);
        assert_eq!(sample.kind, PacketKind::MotionSample);
        assert_eq!(MAX_DATAGRAM_BYTES, 1024);
    }

    #[test]
    fn parse_validated_datagram_accepts_valid_fixture() {
        let sample = parse_validated_datagram(VALID_FIXTURE.as_bytes()).expect("valid fixture");

        assert_eq!(u8::from(sample.protocol_version), PROTOCOL_VERSION);
        assert_eq!(sample.kind, PacketKind::MotionSample);
    }

    #[test]
    fn parse_validated_datagram_rejects_malformed_json() {
        let err = parse_validated_datagram(br#"{"#).expect_err("malformed json must fail");

        assert!(matches!(err, ProtocolParseError::MalformedJson));
    }

    #[test]
    fn parse_validated_datagram_rejects_semantically_invalid_fixture() {
        let err = parse_validated_datagram(INVALID_OUT_OF_RANGE.as_bytes())
            .expect_err("semantic validation must fail");

        assert!(matches!(
            err,
            ProtocolParseError::InvalidSemantics(
                ProtocolValidationError::VectorComponentOutOfRange { .. }
            )
        ));
    }

    #[test]
    fn parse_validated_datagram_rejects_unknown_property() {
        let err = parse_validated_datagram(INVALID_UNKNOWN_PROPERTY.as_bytes())
            .expect_err("unknown property must fail");

        assert!(matches!(err, ProtocolParseError::InvalidStructure));
    }

    #[test]
    fn parse_validated_datagram_rejects_unsupported_version() {
        let err = parse_validated_datagram(INVALID_UNSUPPORTED_VERSION.as_bytes())
            .expect_err("unsupported version must fail");

        assert!(matches!(err, ProtocolParseError::InvalidStructure));
    }

    #[test]
    fn parse_validated_datagram_rejects_datagram_larger_than_limit() {
        let oversized = vec![b'a'; MAX_DATAGRAM_BYTES + 1];

        let err = parse_validated_datagram(&oversized).expect_err("oversized datagram must fail");

        assert!(matches!(
            err,
            ProtocolParseError::DatagramTooLarge {
                max: MAX_DATAGRAM_BYTES,
                ..
            }
        ));
        if let ProtocolParseError::DatagramTooLarge { actual, .. } = err {
            assert_eq!(actual, MAX_DATAGRAM_BYTES + 1);
        }
    }

    #[test]
    fn invalid_fixtures_are_rejected_or_invalidated() {
        let fixtures = [
            INVALID_UNSUPPORTED_VERSION,
            INVALID_MISSING_FIELD,
            INVALID_OUT_OF_RANGE,
            INVALID_UNKNOWN_PROPERTY,
        ];

        for fixture in fixtures {
            if let Ok(sample) = serde_json::from_str::<MotionSampleV1>(fixture) {
                assert!(
                    sample.validate().is_err(),
                    "fixture should be invalid: {fixture}"
                );
            }
        }
    }

    #[test]
    fn rust_serialization_matches_json_fixture_field_names() {
        let sample: MotionSampleV1 = serde_json::from_str(VALID_FIXTURE).expect("valid fixture");
        let serialized = serde_json::to_value(&sample).expect("serialize sample");
        let fixture_value: Value = serde_json::from_str(VALID_FIXTURE).expect("fixture json");

        assert_eq!(serialized, fixture_value);
        assert_eq!(serialized["kind"], json!("motion_sample"));
        assert_eq!(serialized["protocolVersion"], json!(1));
        assert!(serialized.get("protocol_version").is_none());
    }

    #[test]
    fn rust_round_trip_preserves_sample() {
        let sample: MotionSampleV1 = serde_json::from_str(VALID_FIXTURE).expect("valid fixture");
        let serialized = serde_json::to_string(&sample).expect("serialize sample");
        let round_tripped: MotionSampleV1 =
            serde_json::from_str(&serialized).expect("round trip deserialize");

        round_tripped
            .validate()
            .expect("round tripped sample should validate");
        assert_eq!(round_tripped, sample);
    }

    #[test]
    fn out_of_range_validation_reports_axis() {
        let sample: MotionSampleV1 = serde_json::from_str(INVALID_OUT_OF_RANGE)
            .expect("out-of-range fixture still deserializes so explicit validation can reject it");

        let err = sample
            .validate()
            .expect_err("fixture should fail validation");

        assert_eq!(
            err,
            ProtocolValidationError::VectorComponentOutOfRange {
                field_name: "linearAccelerationMps2",
                axis: "y",
                min: -200.0,
                max: 200.0,
                actual: -245.0,
            }
        );
    }
}
