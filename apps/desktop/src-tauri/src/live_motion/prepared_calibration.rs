use crate::{
    calibration::{
        provenance::{CalibrationProfileFingerprintV1, FingerprintError},
        subtract_vectors, validate_finite_vector, validated_calibration_profile, CalibrationError,
        CalibrationProfileV1, UnitQuaternion,
    },
    motion_filtering::estimator::CalibratedMotionSample,
    protocol::{MotionSampleV1, Vector3},
};

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedCalibrationV1 {
    linear_bias: Vector3,
    angular_bias: Vector3,
    rotation: UnitQuaternion,
    fingerprint: CalibrationProfileFingerprintV1,
}

impl PreparedCalibrationV1 {
    pub fn new(profile: &CalibrationProfileV1) -> Result<Self, CalibrationError> {
        let validated = validated_calibration_profile(profile)?;
        let fingerprint =
            CalibrationProfileFingerprintV1::for_profile(&validated).map_err(|err| match err {
                FingerprintError::Calibration(err) => err,
                other => CalibrationError::InvalidProfile(other.to_string()),
            })?;
        Ok(Self {
            linear_bias: validated.device_frame_linear_acceleration_bias_mps2,
            angular_bias: validated.device_frame_angular_velocity_bias_rad_s,
            rotation: validated.device_to_leveled_quaternion,
            fingerprint,
        })
    }

    pub fn fingerprint(&self) -> &CalibrationProfileFingerprintV1 {
        &self.fingerprint
    }

    pub fn apply_linear(&self, linear_raw: &Vector3) -> Result<Vector3, CalibrationError> {
        validate_finite_vector(linear_raw, "linearRaw")?;
        Ok(self
            .rotation
            .rotate_vector(&subtract_vectors(linear_raw, &self.linear_bias)))
    }

    pub fn apply_angular(&self, angular_raw: &Vector3) -> Result<Vector3, CalibrationError> {
        validate_finite_vector(angular_raw, "angularRaw")?;
        Ok(self
            .rotation
            .rotate_vector(&subtract_vectors(angular_raw, &self.angular_bias)))
    }

    pub fn apply_gravity(&self, gravity_raw: &Vector3) -> Result<Vector3, CalibrationError> {
        validate_finite_vector(gravity_raw, "gravityRaw")?;
        Ok(self.rotation.rotate_vector(gravity_raw))
    }

    pub fn calibrate_sample(
        &self,
        sample: &MotionSampleV1,
    ) -> Result<CalibratedMotionSample, CalibrationError> {
        Ok(CalibratedMotionSample {
            gravity_mps2: self.apply_gravity(&sample.gravity_mps2)?,
            angular_velocity_rad_s: self.apply_angular(&sample.angular_velocity_rad_s)?,
            linear_acceleration_mps2: Some(self.apply_linear(&sample.linear_acceleration_mps2)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::{
        apply_calibration_to_angular, apply_calibration_to_gravity, apply_calibration_to_linear,
    };
    use crate::motion_filtering::synthetic::synthetic_suite;

    #[test]
    fn prepared_matches_b2_apply_apis_repeatedly() {
        let fixture = synthetic_suite()
            .unwrap()
            .into_iter()
            .find(|item| item.name == "mounting_bias_b2")
            .unwrap();
        let prepared = PreparedCalibrationV1::new(&fixture.calibration_profile).unwrap();
        for sample in fixture.raw_samples.iter().take(16) {
            assert_eq!(
                prepared.apply_gravity(&sample.gravity_mps2).unwrap(),
                apply_calibration_to_gravity(&fixture.calibration_profile, &sample.gravity_mps2)
                    .unwrap()
            );
            assert_eq!(
                prepared
                    .apply_angular(&sample.angular_velocity_rad_s)
                    .unwrap(),
                apply_calibration_to_angular(
                    &fixture.calibration_profile,
                    &sample.angular_velocity_rad_s
                )
                .unwrap()
            );
            assert_eq!(
                prepared
                    .apply_linear(&sample.linear_acceleration_mps2)
                    .unwrap(),
                apply_calibration_to_linear(
                    &fixture.calibration_profile,
                    &sample.linear_acceleration_mps2
                )
                .unwrap()
            );
        }
    }

    #[test]
    fn prepared_rejects_invalid_profile_once() {
        let mut profile = synthetic_suite().unwrap().remove(0).calibration_profile;
        profile.yaw_calibrated = true;
        assert!(PreparedCalibrationV1::new(&profile).is_err());
    }
}
