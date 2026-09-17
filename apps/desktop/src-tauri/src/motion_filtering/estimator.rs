use crate::protocol::Vector3;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const GRAVITY_NORM_EPSILON: f64 = 1e-9;
const SMALL_ROTATION_EPSILON: f64 = 1e-12;

#[derive(Debug, Clone, PartialEq)]
pub enum EstimatorError {
    InvalidDt,
    InvalidTau,
    NonFiniteVector(&'static str),
    DegenerateVector(&'static str),
}

impl fmt::Display for EstimatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDt => f.write_str("dtSeconds must be positive and finite"),
            Self::InvalidTau => f.write_str("tauSeconds must be positive and finite"),
            Self::NonFiniteVector(name) => write!(f, "{name} must contain finite components"),
            Self::DegenerateVector(name) => write!(f, "{name} norm is too small"),
        }
    }
}
impl std::error::Error for EstimatorError {}

#[derive(Debug, Clone, PartialEq)]
pub struct CalibratedMotionSample {
    pub gravity_mps2: Vector3,
    pub angular_velocity_rad_s: Vector3,
    pub linear_acceleration_mps2: Option<Vector3>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AngleEstimate {
    pub gravity_direction: Vector3,
    pub roll_rad: f64,
    pub pitch_rad: f64,
    pub yaw_available: bool,
}

pub trait Estimator {
    fn initialize(
        &mut self,
        sample: &CalibratedMotionSample,
    ) -> Result<AngleEstimate, EstimatorError>;
    fn update(
        &mut self,
        sample: &CalibratedMotionSample,
        dt_seconds: f64,
    ) -> Result<AngleEstimate, EstimatorError>;
}

#[derive(Debug, Default)]
pub struct GravityOnlyEstimator;
impl GravityOnlyEstimator {
    pub fn new() -> Self {
        Self
    }
}
impl Estimator for GravityOnlyEstimator {
    fn initialize(
        &mut self,
        sample: &CalibratedMotionSample,
    ) -> Result<AngleEstimate, EstimatorError> {
        estimate_from_gravity(&sample.gravity_mps2)
    }
    fn update(
        &mut self,
        sample: &CalibratedMotionSample,
        dt_seconds: f64,
    ) -> Result<AngleEstimate, EstimatorError> {
        validate_dt(dt_seconds)?;
        estimate_from_gravity(&sample.gravity_mps2)
    }
}

#[derive(Debug)]
pub struct LowPassGravityEstimator {
    tau_seconds: f64,
    filtered: Option<Vector3>,
}
impl LowPassGravityEstimator {
    pub fn new(tau_seconds: f64) -> Result<Self, EstimatorError> {
        validate_tau(tau_seconds)?;
        Ok(Self {
            tau_seconds,
            filtered: None,
        })
    }
}
impl Estimator for LowPassGravityEstimator {
    fn initialize(
        &mut self,
        sample: &CalibratedMotionSample,
    ) -> Result<AngleEstimate, EstimatorError> {
        validate_finite_vector(&sample.gravity_mps2, "gravityCalibrated")?;
        self.filtered = Some(sample.gravity_mps2.clone());
        estimate_from_gravity(&sample.gravity_mps2)
    }
    fn update(
        &mut self,
        sample: &CalibratedMotionSample,
        dt_seconds: f64,
    ) -> Result<AngleEstimate, EstimatorError> {
        validate_dt(dt_seconds)?;
        validate_finite_vector(&sample.gravity_mps2, "gravityCalibrated")?;
        let previous = self
            .filtered
            .clone()
            .unwrap_or_else(|| sample.gravity_mps2.clone());
        let x = dt_seconds / self.tau_seconds;
        let injection = -(-x).exp_m1();
        let retention = 1.0 - injection;
        let filtered = add(
            &scale(&previous, retention),
            &scale(&sample.gravity_mps2, injection),
        );
        let estimate = estimate_from_gravity_named(&filtered, "gFiltered")?;
        self.filtered = Some(filtered);
        Ok(estimate)
    }
}

#[derive(Debug)]
pub struct ComplementaryTiltEstimator {
    correction_tau_seconds: f64,
    direction: Option<Vector3>,
}
impl ComplementaryTiltEstimator {
    pub fn new(correction_tau_seconds: f64) -> Result<Self, EstimatorError> {
        validate_tau(correction_tau_seconds)?;
        Ok(Self {
            correction_tau_seconds,
            direction: None,
        })
    }
}
impl Estimator for ComplementaryTiltEstimator {
    fn initialize(
        &mut self,
        sample: &CalibratedMotionSample,
    ) -> Result<AngleEstimate, EstimatorError> {
        let d = normalize_named(&sample.gravity_mps2, "gravityCalibrated")?;
        self.direction = Some(d.clone());
        Ok(estimate_from_direction(d))
    }
    fn update(
        &mut self,
        sample: &CalibratedMotionSample,
        dt_seconds: f64,
    ) -> Result<AngleEstimate, EstimatorError> {
        validate_dt(dt_seconds)?;
        validate_finite_vector(
            &sample.angular_velocity_rad_s,
            "angularVelocityCalibratedRadS",
        )?;
        let previous = self.direction.clone().unwrap_or_else(|| {
            normalize_named(&sample.gravity_mps2, "gravityCalibrated").unwrap_or(Vector3 {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            })
        });
        let rotation = scale(&sample.angular_velocity_rad_s, -dt_seconds);
        let predicted = rotate_by_rotation_vector(&previous, &rotation)?;
        let measured = normalize_named(&sample.gravity_mps2, "gravityCalibrated")?;
        let beta = -(-(dt_seconds / self.correction_tau_seconds)).exp_m1();
        let mixed = add(&scale(&predicted, 1.0 - beta), &scale(&measured, beta));
        let direction = match normalize_named(&mixed, "dMixed") {
            Ok(direction) => direction,
            Err(EstimatorError::DegenerateVector(_)) if dot(&predicted, &measured) < -0.999_999 => {
                measured
            }
            Err(err) => return Err(err),
        };
        self.direction = Some(direction.clone());
        Ok(estimate_from_direction(direction))
    }
}

pub fn estimate_from_gravity(gravity: &Vector3) -> Result<AngleEstimate, EstimatorError> {
    estimate_from_gravity_named(gravity, "gravityCalibrated")
}
fn estimate_from_gravity_named(
    gravity: &Vector3,
    name: &'static str,
) -> Result<AngleEstimate, EstimatorError> {
    Ok(estimate_from_direction(normalize_named(gravity, name)?))
}
pub fn estimate_from_direction(d: Vector3) -> AngleEstimate {
    AngleEstimate {
        roll_rad: d.x.atan2((d.y * d.y + d.z * d.z).sqrt()),
        pitch_rad: d.y.atan2(-d.z),
        gravity_direction: d,
        yaw_available: false,
    }
}
pub fn angular_error_rad(a: &Vector3, b: &Vector3) -> f64 {
    dot(a, b).clamp(-1.0, 1.0).acos()
}
pub fn validate_dt(dt: f64) -> Result<(), EstimatorError> {
    if dt.is_finite() && dt > 0.0 {
        Ok(())
    } else {
        Err(EstimatorError::InvalidDt)
    }
}
fn validate_tau(tau: f64) -> Result<(), EstimatorError> {
    if tau.is_finite() && tau > 0.0 {
        Ok(())
    } else {
        Err(EstimatorError::InvalidTau)
    }
}
pub fn normalize_named(v: &Vector3, name: &'static str) -> Result<Vector3, EstimatorError> {
    validate_finite_vector(v, name)?;
    let norm = magnitude(v);
    if norm <= GRAVITY_NORM_EPSILON {
        Err(EstimatorError::DegenerateVector(name))
    } else {
        Ok(scale(v, 1.0 / norm))
    }
}
fn validate_finite_vector(v: &Vector3, name: &'static str) -> Result<(), EstimatorError> {
    if v.x.is_finite() && v.y.is_finite() && v.z.is_finite() {
        Ok(())
    } else {
        Err(EstimatorError::NonFiniteVector(name))
    }
}
pub fn add(a: &Vector3, b: &Vector3) -> Vector3 {
    Vector3 {
        x: a.x + b.x,
        y: a.y + b.y,
        z: a.z + b.z,
    }
}
pub fn sub(a: &Vector3, b: &Vector3) -> Vector3 {
    Vector3 {
        x: a.x - b.x,
        y: a.y - b.y,
        z: a.z - b.z,
    }
}
pub fn scale(v: &Vector3, s: f64) -> Vector3 {
    Vector3 {
        x: v.x * s,
        y: v.y * s,
        z: v.z * s,
    }
}
pub fn dot(a: &Vector3, b: &Vector3) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}
pub fn cross(a: &Vector3, b: &Vector3) -> Vector3 {
    Vector3 {
        x: a.y * b.z - a.z * b.y,
        y: a.z * b.x - a.x * b.z,
        z: a.x * b.y - a.y * b.x,
    }
}
pub fn magnitude(v: &Vector3) -> f64 {
    dot(v, v).sqrt()
}

pub fn rotate_by_rotation_vector(
    v: &Vector3,
    rotation: &Vector3,
) -> Result<Vector3, EstimatorError> {
    let theta = magnitude(rotation);
    if !theta.is_finite() {
        return Err(EstimatorError::NonFiniteVector("rotationVector"));
    }
    let rotated = if theta <= SMALL_ROTATION_EPSILON {
        v.clone()
    } else {
        let axis = scale(rotation, 1.0 / theta);
        let (sin_t, cos_t) = theta.sin_cos();
        add(
            &add(&scale(v, cos_t), &scale(&cross(&axis, v), sin_t)),
            &scale(&axis, dot(&axis, v) * (1.0 - cos_t)),
        )
    };
    normalize_named(&rotated, "dPredicted")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(x: f64, y: f64, z: f64) -> Vector3 {
        Vector3 { x, y, z }
    }
    fn sample(g: Vector3, w: Vector3) -> CalibratedMotionSample {
        CalibratedMotionSample {
            gravity_mps2: g,
            angular_velocity_rad_s: w,
            linear_acceleration_mps2: None,
        }
    }
    #[test]
    fn roll_pitch_signs() {
        let e = estimate_from_gravity(&v(0.0, 0.0, -9.8)).unwrap();
        assert!(e.roll_rad.abs() < 1e-12 && e.pitch_rad.abs() < 1e-12);
        assert!(estimate_from_gravity(&v(1.0, 0.0, -9.8)).unwrap().roll_rad > 0.0);
        assert!(estimate_from_gravity(&v(0.0, 1.0, -9.8)).unwrap().pitch_rad > 0.0);
    }
    #[test]
    fn invalid_dt_rejected() {
        let s = sample(v(0.0, 0.0, -9.8), v(0.0, 0.0, 0.0));
        for dt in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut gravity = GravityOnlyEstimator::new();
            gravity.initialize(&s).unwrap();
            assert_eq!(gravity.update(&s, dt), Err(EstimatorError::InvalidDt));

            let mut low_pass = LowPassGravityEstimator::new(1.0).unwrap();
            low_pass.initialize(&s).unwrap();
            assert_eq!(low_pass.update(&s, dt), Err(EstimatorError::InvalidDt));

            let mut complementary = ComplementaryTiltEstimator::new(1.0).unwrap();
            complementary.initialize(&s).unwrap();
            assert_eq!(complementary.update(&s, dt), Err(EstimatorError::InvalidDt));
        }
    }
    #[test]
    fn low_pass_tau_and_response() {
        assert!(LowPassGravityEstimator::new(0.0).is_err());
        let mut e = LowPassGravityEstimator::new(1.0).unwrap();
        e.initialize(&sample(v(0.0, 0.0, -10.0), v(0.0, 0.0, 0.0)))
            .unwrap();
        let out = e
            .update(&sample(v(10.0, 0.0, 0.0), v(0.0, 0.0, 0.0)), 1.0)
            .unwrap();
        assert!(out.gravity_direction.x > 0.5);
        assert!((magnitude(&out.gravity_direction) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn low_pass_dt_equal_tau_matches_analytic_alpha_before_normalization() {
        let tau = 0.75;
        let alpha = 1.0 - std::f64::consts::E.powi(-1);
        let mut estimator = LowPassGravityEstimator::new(tau).unwrap();
        estimator
            .initialize(&sample(v(0.0, 0.0, -1.0), v(0.0, 0.0, 0.0)))
            .unwrap();
        let estimate = estimator
            .update(&sample(v(1.0, 0.0, 0.0), v(0.0, 0.0, 0.0)), tau)
            .unwrap();
        let filtered = estimator.filtered.as_ref().unwrap();
        assert_close(filtered.x, alpha, 1e-15);
        assert_close(filtered.z, -(1.0 - alpha), 1e-15);
        assert_close(
            estimate.gravity_direction.x,
            alpha / magnitude(filtered),
            1e-15,
        );
        assert_unit(&estimate.gravity_direction);
    }

    #[test]
    fn low_pass_single_update_matches_subdivisions() {
        let total_dt = 0.8;
        let tau = 0.3;
        let initial = sample(v(0.0, 0.0, -1.0), v(0.0, 0.0, 0.0));
        let target = sample(v(0.8, 0.0, -0.6), v(0.0, 0.0, 0.0));

        let mut single = LowPassGravityEstimator::new(tau).unwrap();
        single.initialize(&initial).unwrap();
        let single_out = single.update(&target, total_dt).unwrap();

        let mut subdivided = LowPassGravityEstimator::new(tau).unwrap();
        subdivided.initialize(&initial).unwrap();
        let mut subdivided_out = subdivided.update(&target, total_dt / 8.0).unwrap();
        for _ in 1..8 {
            subdivided_out = subdivided.update(&target, total_dt / 8.0).unwrap();
        }

        assert_vector_close(
            &single.filtered.unwrap(),
            &subdivided.filtered.unwrap(),
            2e-15,
        );
        assert_vector_close(
            &single_out.gravity_direction,
            &subdivided_out.gravity_direction,
            2e-15,
        );
    }

    #[test]
    fn complementary_uses_negative_body_sign() {
        let mut e = ComplementaryTiltEstimator::new(1000.0).unwrap();
        e.initialize(&sample(v(0.0, 0.0, -9.8), v(0.0, 0.0, 0.0)))
            .unwrap();
        let out = e
            .update(&sample(v(0.0, 0.0, -9.8), v(0.0, 1.0, 0.0)), 0.1)
            .unwrap();
        assert!(out.gravity_direction.x > 0.0);
        assert!(!out.yaw_available);
    }

    #[test]
    fn complementary_propagates_known_body_rate_with_documented_sign() {
        let dt = 0.2;
        let mut estimator = ComplementaryTiltEstimator::new(1.0e15).unwrap();
        estimator
            .initialize(&sample(v(0.0, 0.0, -1.0), v(0.0, 0.0, 0.0)))
            .unwrap();
        let out = estimator
            .update(&sample(v(0.0, 0.0, -1.0), v(0.0, 1.0, 0.0)), dt)
            .unwrap();
        assert_vector_close(&out.gravity_direction, &v(dt.sin(), 0.0, -dt.cos()), 1e-12);
        assert_unit(&out.gravity_direction);
    }

    #[test]
    fn complementary_beta_matches_analytic_mixing_for_different_dt_and_tau() {
        for (dt, tau) in [(0.1_f64, 1.0_f64), (0.4_f64, 0.2_f64)] {
            let measured = direction_from_roll(30_f64.to_radians());
            let beta = -(-(dt / tau)).exp_m1();
            let predicted = v(0.0, 0.0, -1.0);
            let expected = normalize_named(
                &add(&scale(&predicted, 1.0 - beta), &scale(&measured, beta)),
                "expected",
            )
            .unwrap();

            let mut estimator = ComplementaryTiltEstimator::new(tau).unwrap();
            estimator
                .initialize(&sample(predicted.clone(), v(0.0, 0.0, 0.0)))
                .unwrap();
            let out = estimator
                .update(&sample(measured, v(0.0, 0.0, 0.0)), dt)
                .unwrap();
            assert_vector_close(&out.gravity_direction, &expected, 1e-15);
        }
    }

    #[test]
    fn complementary_gyro_drift_recovers_toward_gravity_deterministically() {
        let mut estimator = ComplementaryTiltEstimator::new(0.2).unwrap();
        let level = sample(v(0.0, 0.0, -1.0), v(0.0, 0.0, 0.0));
        estimator.initialize(&level).unwrap();
        let drift_sample = sample(v(0.0, 0.0, -1.0), v(0.0, 0.25, 0.0));
        let mut out = estimator.update(&drift_sample, 0.1).unwrap();
        for _ in 1..8 {
            out = estimator.update(&drift_sample, 0.1).unwrap();
        }
        let drift_error = angular_error_rad(&out.gravity_direction, &level.gravity_mps2);
        assert_close(drift_error, 0.037836143628739514, 1e-12);

        for _ in 0..12 {
            out = estimator.update(&level, 0.1).unwrap();
        }
        let recovered_error = angular_error_rad(&out.gravity_direction, &level.gravity_mps2);
        assert_close(recovered_error, 0.00009378939064329948, 1e-12);
    }

    #[test]
    fn complementary_handles_antipodal_and_degenerate_cases_without_bad_state() {
        let mut antipodal = ComplementaryTiltEstimator::new(1.0 / std::f64::consts::LN_2).unwrap();
        antipodal
            .initialize(&sample(v(0.0, 0.0, -1.0), v(0.0, 0.0, 0.0)))
            .unwrap();
        let out = antipodal
            .update(&sample(v(0.0, 0.0, 1.0), v(0.0, 0.0, 0.0)), 1.0)
            .unwrap();
        assert_vector_close(&out.gravity_direction, &v(0.0, 0.0, 1.0), 1e-12);
        assert_unit(&out.gravity_direction);

        let mut degenerate = ComplementaryTiltEstimator::new(1.0).unwrap();
        degenerate
            .initialize(&sample(v(0.0, 0.0, -1.0), v(0.0, 0.0, 0.0)))
            .unwrap();
        assert_eq!(
            degenerate.update(&sample(v(0.0, 0.0, 0.0), v(0.0, 0.0, 0.0)), 0.1),
            Err(EstimatorError::DegenerateVector("gravityCalibrated"))
        );
    }

    #[test]
    fn estimators_keep_normalized_state_after_updates() {
        let initial = sample(v(0.0, 0.0, -2.0), v(0.0, 0.0, 0.0));
        let next = sample(v(0.4, 0.2, -1.7), v(0.1, -0.2, 0.05));

        let mut gravity = GravityOnlyEstimator::new();
        gravity.initialize(&initial).unwrap();
        assert_unit(&gravity.update(&next, 0.2).unwrap().gravity_direction);

        let mut low_pass = LowPassGravityEstimator::new(0.4).unwrap();
        low_pass.initialize(&initial).unwrap();
        assert_unit(&low_pass.update(&next, 0.2).unwrap().gravity_direction);

        let mut complementary = ComplementaryTiltEstimator::new(0.4).unwrap();
        complementary.initialize(&initial).unwrap();
        let out = complementary.update(&next, 0.2).unwrap();
        assert_unit(&out.gravity_direction);
        assert_unit(complementary.direction.as_ref().unwrap());
    }

    #[test]
    fn large_positive_dt_is_finite() {
        let mut e = ComplementaryTiltEstimator::new(1.0).unwrap();
        let s = sample(v(0.0, 0.0, -9.8), v(0.0, 0.0, 0.0));
        e.initialize(&s).unwrap();
        let out = e.update(&s, 1.0e6).unwrap();
        assert!(out.roll_rad.is_finite() && out.pitch_rad.is_finite());
    }

    fn direction_from_roll(roll: f64) -> Vector3 {
        v(roll.sin(), 0.0, -roll.cos())
    }

    fn assert_unit(vector: &Vector3) {
        assert_close(magnitude(vector), 1.0, 1e-12);
        assert!(vector.x.is_finite() && vector.y.is_finite() && vector.z.is_finite());
    }

    fn assert_vector_close(actual: &Vector3, expected: &Vector3, tolerance: f64) {
        assert_close(actual.x, expected.x, tolerance);
        assert_close(actual.y, expected.y, tolerance);
        assert_close(actual.z, expected.z, tolerance);
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected} +/- {tolerance}, got {actual}"
        );
    }
}
