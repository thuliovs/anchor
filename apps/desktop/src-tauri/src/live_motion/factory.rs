use crate::motion_filtering::{
    estimator::{
        ComplementaryTiltEstimator, Estimator, GravityOnlyEstimator, LowPassGravityEstimator,
    },
    selection::{PolicyCandidate, TiltEstimatorPolicyV2},
};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum FactoryError {
    InvalidPolicy(String),
    InvalidEstimator(String),
}

impl fmt::Display for FactoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy(msg) | Self::InvalidEstimator(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for FactoryError {}

#[derive(Debug, Clone, PartialEq)]
pub struct EstimatorFactory {
    policy: TiltEstimatorPolicyV2,
}

impl EstimatorFactory {
    pub fn new(policy: TiltEstimatorPolicyV2) -> Result<Self, FactoryError> {
        policy
            .validate()
            .map_err(|err| FactoryError::InvalidPolicy(err.to_string()))?;
        Ok(Self { policy })
    }

    pub fn policy_version(&self) -> u8 {
        self.policy.version
    }

    pub fn build(&self) -> Result<Box<dyn Estimator>, FactoryError> {
        match self.policy.candidate {
            PolicyCandidate::GravityNoAdditionalAnchorFilter => {
                Ok(Box::new(GravityOnlyEstimator::new()))
            }
            PolicyCandidate::LowPassGravity => {
                let tau_ms = single_param(&self.policy, "tauMs")?;
                Ok(Box::new(
                    LowPassGravityEstimator::new(tau_ms / 1000.0)
                        .map_err(|err| FactoryError::InvalidEstimator(err.to_string()))?,
                ))
            }
            PolicyCandidate::ComplementaryGravityGyro => {
                let tau_ms = single_param(&self.policy, "correctionTauMs")?;
                Ok(Box::new(
                    ComplementaryTiltEstimator::new(tau_ms / 1000.0)
                        .map_err(|err| FactoryError::InvalidEstimator(err.to_string()))?,
                ))
            }
        }
    }
}

fn single_param(policy: &TiltEstimatorPolicyV2, name: &str) -> Result<f64, FactoryError> {
    match (
        policy.parameters.len(),
        policy.parameters.get(name).copied(),
    ) {
        (1, Some(value)) if value.is_finite() && value > 0.0 => Ok(value),
        _ => Err(FactoryError::InvalidPolicy(format!(
            "policy parameters must contain exactly positive finite {name}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        calibration::provenance::CalibrationProfileFingerprintV1,
        motion_filtering::selection::POLICY_SOURCE,
    };
    use std::collections::BTreeMap;

    fn fp() -> CalibrationProfileFingerprintV1 {
        CalibrationProfileFingerprintV1 {
            version: 1,
            algorithm: "sha256".to_owned(),
            digest: "0".repeat(64),
        }
    }

    fn policy(
        candidate: PolicyCandidate,
        parameters: BTreeMap<String, f64>,
    ) -> TiltEstimatorPolicyV2 {
        TiltEstimatorPolicyV2 {
            version: 2,
            candidate,
            parameters,
            yaw_available: false,
            source: POLICY_SOURCE.to_owned(),
            calibration_profile_fingerprint: fp(),
        }
    }

    #[test]
    fn builds_all_three_estimators_and_rebuilds_cleanly() {
        let gravity = EstimatorFactory::new(policy(
            PolicyCandidate::GravityNoAdditionalAnchorFilter,
            BTreeMap::new(),
        ))
        .unwrap();
        assert!(gravity.build().is_ok());
        assert!(gravity.build().is_ok());

        let mut p = BTreeMap::new();
        p.insert("tauMs".to_owned(), 25.0);
        assert!(
            EstimatorFactory::new(policy(PolicyCandidate::LowPassGravity, p))
                .unwrap()
                .build()
                .is_ok()
        );

        let mut p = BTreeMap::new();
        p.insert("correctionTauMs".to_owned(), 50.0);
        assert!(
            EstimatorFactory::new(policy(PolicyCandidate::ComplementaryGravityGyro, p))
                .unwrap()
                .build()
                .is_ok()
        );
    }

    #[test]
    fn rejects_invalid_tau() {
        let mut p = BTreeMap::new();
        p.insert("tauMs".to_owned(), f64::NAN);
        assert!(EstimatorFactory::new(policy(PolicyCandidate::LowPassGravity, p)).is_err());
    }
}
