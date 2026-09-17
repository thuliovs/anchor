use crate::{
    calibration::{
        calibrate_dataset_str_at, CalibrationError, CalibrationProfileV1, UnitQuaternion,
    },
    dataset::DEFAULT_MOUNTING_CONVENTION,
    motion_filtering::estimator::{add, cross, normalize_named, scale},
    protocol::{MotionSampleV1, PacketKind, ProtocolVersion, Vector3},
};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

const G: f64 = 9.80665;

#[derive(Debug, Clone)]
pub struct SyntheticFixture {
    pub name: String,
    pub raw_samples: Vec<MotionSampleV1>,
    pub calibration_profile: CalibrationProfileV1,
    pub calibration_kind: String,
    pub truth: Vec<GroundTruthSample>,
    pub elapsed_seconds: Vec<f64>,
    pub event: Option<FixtureEvent>,
    pub seed: Option<u64>,
    pub canonical_samples: Vec<CanonicalSample>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundTruthSample {
    pub gravity_direction: Vector3,
    pub roll_rad: f64,
    pub pitch_rad: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalSample {
    pub gravity_mps2: Vector3,
    pub angular_velocity_rad_s: Vector3,
    pub linear_acceleration_mps2: Vector3,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FixtureEvent {
    Step {
        axis: TiltAxis,
        onset_seconds: f64,
        initial_angle_rad: f64,
        target_angle_rad: f64,
        settling_band_deg: f64,
    },
    Sine {
        axis: TiltAxis,
        start_seconds: f64,
        end_seconds: f64,
        frequency_hz: f64,
    },
    GravityContamination {
        start_seconds: f64,
        end_seconds: f64,
        recovery_band_deg: f64,
        minimum_recovery_hold_seconds: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiltAxis {
    Roll,
    Pitch,
}

impl SyntheticFixture {
    pub fn dt_seconds(&self) -> Vec<f64> {
        self.elapsed_seconds
            .windows(2)
            .map(|p| p[1] - p[0])
            .collect()
    }

    pub fn observed_duration_us(&self) -> u64 {
        ((self.elapsed_seconds.last().copied().unwrap_or(0.0)
            - self.elapsed_seconds.first().copied().unwrap_or(0.0))
            * 1_000_000.0)
            .round() as u64
    }
}

#[derive(Debug, Clone, Copy)]
struct TiltTrajectory {
    roll_rad: f64,
    pitch_rad: f64,
    yaw_rad: f64,
}

#[derive(Debug, Clone)]
struct RawFixtureOptions {
    event: Option<FixtureEvent>,
    calibration_profile: CalibrationProfileV1,
    calibration_kind: String,
    gyro_bias: Option<Vector3>,
    gravity_pulse: Option<(f64, f64, Vector3)>,
}

#[derive(Debug, Clone, Copy)]
pub struct Quaternion {
    w: f64,
    x: f64,
    y: f64,
    z: f64,
}

pub fn synthetic_suite() -> Result<Vec<SyntheticFixture>, crate::motion_filtering::EvaluationError>
{
    let identity = identity_profile()?;
    Ok(vec![
        build_simple_fixture(
            "level_rest",
            regular_times(60, 1.0),
            |_| tilt(0.0, 0.0, 0.0),
            None,
            identity.clone(),
        ),
        build_simple_fixture(
            "known_tilt_rest",
            regular_times(60, 1.0),
            |_| tilt(10_f64.to_radians(), 8_f64.to_radians(), 0.0),
            None,
            identity.clone(),
        ),
        build_simple_fixture(
            "roll_step_positive",
            regular_times(120, 2.0),
            |t| tilt(if t >= 0.5 { 20_f64.to_radians() } else { 0.0 }, 0.0, 0.0),
            Some(FixtureEvent::Step {
                axis: TiltAxis::Roll,
                onset_seconds: 0.5,
                initial_angle_rad: 0.0,
                target_angle_rad: 20_f64.to_radians(),
                settling_band_deg: 2.0,
            }),
            identity.clone(),
        ),
        build_simple_fixture(
            "roll_step_negative",
            regular_times(120, 2.0),
            |t| tilt(if t >= 0.5 { -20_f64.to_radians() } else { 0.0 }, 0.0, 0.0),
            Some(FixtureEvent::Step {
                axis: TiltAxis::Roll,
                onset_seconds: 0.5,
                initial_angle_rad: 0.0,
                target_angle_rad: -20_f64.to_radians(),
                settling_band_deg: 2.0,
            }),
            identity.clone(),
        ),
        build_simple_fixture(
            "pitch_step_positive",
            regular_times(120, 2.0),
            |t| tilt(0.0, if t >= 0.5 { 20_f64.to_radians() } else { 0.0 }, 0.0),
            Some(FixtureEvent::Step {
                axis: TiltAxis::Pitch,
                onset_seconds: 0.5,
                initial_angle_rad: 0.0,
                target_angle_rad: 20_f64.to_radians(),
                settling_band_deg: 2.0,
            }),
            identity.clone(),
        ),
        build_simple_fixture(
            "pitch_step_negative",
            regular_times(120, 2.0),
            |t| tilt(0.0, if t >= 0.5 { -20_f64.to_radians() } else { 0.0 }, 0.0),
            Some(FixtureEvent::Step {
                axis: TiltAxis::Pitch,
                onset_seconds: 0.5,
                initial_angle_rad: 0.0,
                target_angle_rad: -20_f64.to_radians(),
                settling_band_deg: 2.0,
            }),
            identity.clone(),
        ),
        build_simple_fixture(
            "roll_ramp",
            regular_times(120, 2.0),
            |t| tilt((t / 2.0) * 25_f64.to_radians(), 0.0, 0.0),
            None,
            identity.clone(),
        ),
        build_simple_fixture(
            "roll_sine",
            regular_times(180, 3.0),
            |t| {
                tilt(
                    15_f64.to_radians() * (2.0 * std::f64::consts::PI * t).sin(),
                    0.0,
                    0.0,
                )
            },
            Some(FixtureEvent::Sine {
                axis: TiltAxis::Roll,
                start_seconds: 0.0,
                end_seconds: 3.0,
                frequency_hz: 1.0,
            }),
            identity.clone(),
        ),
        build_simple_fixture(
            "pitch_sine",
            regular_times(180, 3.0),
            |t| {
                tilt(
                    0.0,
                    15_f64.to_radians() * (2.0 * std::f64::consts::PI * t).sin(),
                    0.0,
                )
            },
            Some(FixtureEvent::Sine {
                axis: TiltAxis::Pitch,
                start_seconds: 0.0,
                end_seconds: 3.0,
                frequency_hz: 1.0,
            }),
            identity.clone(),
        ),
        build_simple_fixture(
            "combined_roll_pitch",
            regular_times(180, 3.0),
            |t| {
                tilt(
                    12_f64.to_radians() * (2.0 * t).sin(),
                    9_f64.to_radians() * (1.5 * t).cos(),
                    0.0,
                )
            },
            None,
            identity.clone(),
        ),
        build_simple_fixture(
            "pure_yaw",
            regular_times(120, 2.0),
            |t| tilt(0.0, 0.0, t * std::f64::consts::PI),
            None,
            identity.clone(),
        ),
        build_simple_fixture_with_gyro_bias(
            "gyro_bias",
            regular_times(180, 3.0),
            |_| tilt(0.0, 0.0, 0.0),
            Vector3 {
                x: 0.02,
                y: 0.0,
                z: 0.0,
            },
            identity.clone(),
        ),
        build_gravity_pulse_fixture(identity.clone()),
        build_simple_fixture(
            "irregular_dt",
            irregular_times(),
            |t| {
                tilt(
                    8_f64.to_radians() * t.sin(),
                    6_f64.to_radians() * (0.8 * t).cos(),
                    0.0,
                )
            },
            None,
            identity,
        ),
        build_mounting_bias_fixture()?,
    ])
}

fn build_simple_fixture<F>(
    name: &str,
    times: Vec<f64>,
    trajectory: F,
    event: Option<FixtureEvent>,
    profile: CalibrationProfileV1,
) -> SyntheticFixture
where
    F: Fn(f64) -> TiltTrajectory,
{
    build_raw_fixture(
        name,
        times,
        trajectory,
        RawFixtureOptions {
            event,
            calibration_profile: profile,
            calibration_kind: "identity_b2".to_owned(),
            gyro_bias: None,
            gravity_pulse: None,
        },
    )
}

fn build_simple_fixture_with_gyro_bias<F>(
    name: &str,
    times: Vec<f64>,
    trajectory: F,
    gyro_bias: Vector3,
    profile: CalibrationProfileV1,
) -> SyntheticFixture
where
    F: Fn(f64) -> TiltTrajectory,
{
    build_raw_fixture(
        name,
        times,
        trajectory,
        RawFixtureOptions {
            event: None,
            calibration_profile: profile,
            calibration_kind: "identity_b2".to_owned(),
            gyro_bias: Some(gyro_bias),
            gravity_pulse: None,
        },
    )
}

fn build_gravity_pulse_fixture(profile: CalibrationProfileV1) -> SyntheticFixture {
    build_raw_fixture(
        "gravity_pulse",
        regular_times(180, 3.0),
        |_| tilt(0.0, 0.0, 0.0),
        RawFixtureOptions {
            event: Some(FixtureEvent::GravityContamination {
                start_seconds: 1.0,
                end_seconds: 1.2,
                recovery_band_deg: 0.5,
                minimum_recovery_hold_seconds: 0.25,
            }),
            calibration_profile: profile,
            calibration_kind: "identity_b2".to_owned(),
            gyro_bias: None,
            gravity_pulse: Some((
                1.0,
                1.2,
                Vector3 {
                    x: 2.0,
                    y: 0.0,
                    z: 0.0,
                },
            )),
        },
    )
}

fn build_raw_fixture<F>(
    name: &str,
    times: Vec<f64>,
    trajectory: F,
    options: RawFixtureOptions,
) -> SyntheticFixture
where
    F: Fn(f64) -> TiltTrajectory,
{
    let mut canonical_samples = Vec::new();
    let mut raw_samples = Vec::new();
    let mut truth = Vec::new();
    let mut previous_q: Option<Quaternion> = None;
    let mut previous_t = 0.0;
    for (index, &t) in times.iter().enumerate() {
        let declared = trajectory(t);
        let q = q_anchor(declared.roll_rad, declared.pitch_rad, declared.yaw_rad);
        let gravity = q.conjugate().rotate(vec3(0.0, 0.0, -G));
        let direction = normalize_named(&gravity, "syntheticGravity").expect("truth direction");
        let mut angular = if let Some(previous) = previous_q {
            let dt = t - previous_t;
            shortest_rotation_vector(previous.conjugate().mul(q), dt)
        } else {
            vec3(0.0, 0.0, 0.0)
        };
        if let Some(bias) = &options.gyro_bias {
            angular = add(&angular, bias);
        }
        let mut raw_gravity = gravity.clone();
        if let Some((start, end, pulse)) = &options.gravity_pulse {
            if (*start..=*end).contains(&t) {
                raw_gravity = add(&raw_gravity, pulse);
            }
        }
        let linear = vec3(0.0, 0.0, 0.0);
        canonical_samples.push(CanonicalSample {
            gravity_mps2: raw_gravity.clone(),
            angular_velocity_rad_s: angular.clone(),
            linear_acceleration_mps2: linear.clone(),
        });
        raw_samples.push(sample(index as u32, t, raw_gravity, angular, linear));
        truth.push(GroundTruthSample {
            gravity_direction: direction,
            roll_rad: declared.roll_rad,
            pitch_rad: declared.pitch_rad,
        });
        previous_q = Some(q);
        previous_t = t;
    }
    SyntheticFixture {
        name: name.to_owned(),
        raw_samples,
        calibration_profile: options.calibration_profile,
        calibration_kind: options.calibration_kind,
        truth,
        elapsed_seconds: times,
        event: options.event,
        seed: None,
        canonical_samples,
    }
}

fn build_mounting_bias_fixture(
) -> Result<SyntheticFixture, crate::motion_filtering::EvaluationError> {
    let requested_c = q_anchor(7_f64.to_radians(), 5_f64.to_radians(), 0.0);
    let linear_bias = vec3(0.05, -0.02, 0.01);
    let angular_bias = vec3(0.01, -0.005, 0.002);
    let profile = profile_for_mounting(requested_c, linear_bias.clone(), angular_bias.clone())?;
    let c = q_from_profile(&profile.device_to_leveled_quaternion);
    let mut fixture = build_simple_fixture(
        "mounting_bias_b2",
        regular_times(90, 1.5),
        |t| {
            tilt(
                10_f64.to_radians() * t.sin(),
                8_f64.to_radians() * t.cos(),
                0.0,
            )
        },
        None,
        profile.clone(),
    );
    fixture.calibration_kind = "mounting_bias_b2".to_owned();
    fixture.raw_samples.clear();
    for (index, canonical) in fixture.canonical_samples.iter().enumerate() {
        let t = fixture.elapsed_seconds[index];
        let raw_g = c.conjugate().rotate(canonical.gravity_mps2.clone());
        let raw_w = add(
            &angular_bias,
            &c.conjugate()
                .rotate(canonical.angular_velocity_rad_s.clone()),
        );
        let raw_l = add(
            &linear_bias,
            &c.conjugate()
                .rotate(canonical.linear_acceleration_mps2.clone()),
        );
        fixture
            .raw_samples
            .push(sample(index as u32, t, raw_g, raw_w, raw_l));
    }
    Ok(fixture)
}

fn identity_profile() -> Result<CalibrationProfileV1, CalibrationError> {
    profile_for_mounting(
        Quaternion::identity(),
        vec3(0.0, 0.0, 0.0),
        vec3(0.0, 0.0, 0.0),
    )
}

fn profile_for_mounting(
    c: Quaternion,
    linear_bias: Vector3,
    angular_bias: Vector3,
) -> Result<CalibrationProfileV1, CalibrationError> {
    let gravity_device = c.conjugate().rotate(vec3(0.0, 0.0, -G));
    let mut lines = Vec::new();
    lines.push(format!("{{\"recordType\":\"metadata\",\"datasetFormatVersion\":1,\"protocolVersion\":1,\"scenario\":\"stationary\",\"startedAtUtc\":\"2026-09-03T12:00:00Z\",\"expectedSampleRateHz\":60,\"mountingConvention\":\"{}\"}}", DEFAULT_MOUNTING_CONVENTION));
    for i in 0..240_u32 {
        let sample = MotionSampleV1 {
            protocol_version: ProtocolVersion::V1,
            kind: PacketKind::MotionSample,
            session_id: "synthetic-calibration".to_owned(),
            sequence: i,
            session_elapsed_us: u64::from(i) * 16_667,
            linear_acceleration_mps2: linear_bias.clone(),
            gravity_mps2: gravity_device.clone(),
            angular_velocity_rad_s: angular_bias.clone(),
        };
        lines.push(format!(
            "{{\"recordType\":\"sample\",\"receivedElapsedUs\":{},\"sample\":{}}}",
            u64::from(i) * 16_667,
            serde_json::to_string(&sample).expect("sample")
        ));
    }
    lines.push("{\"recordType\":\"summary\",\"completed\":true,\"durationUs\":3983413,\"receivedAcceptedSamples\":240,\"writtenSamples\":240,\"recorderDroppedSamples\":0}".to_owned());
    Ok(calibrate_dataset_str_at(
        &lines.join("\n"),
        "synthetic-stationary.ndjson",
        fixed_time(),
    )?
    .profile)
}

fn sample(
    sequence: u32,
    t: f64,
    gravity: Vector3,
    angular: Vector3,
    linear: Vector3,
) -> MotionSampleV1 {
    MotionSampleV1 {
        protocol_version: ProtocolVersion::V1,
        kind: PacketKind::MotionSample,
        session_id: "synthetic-fixture".to_owned(),
        sequence,
        session_elapsed_us: (t * 1_000_000.0).round() as u64,
        linear_acceleration_mps2: linear,
        gravity_mps2: gravity,
        angular_velocity_rad_s: angular,
    }
}

fn tilt(roll_rad: f64, pitch_rad: f64, yaw_rad: f64) -> TiltTrajectory {
    TiltTrajectory {
        roll_rad,
        pitch_rad,
        yaw_rad,
    }
}
fn vec3(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3 { x, y, z }
}

fn regular_times(samples: usize, duration: f64) -> Vec<f64> {
    (0..samples)
        .map(|i| i as f64 * duration / (samples - 1) as f64)
        .collect()
}
fn irregular_times() -> Vec<f64> {
    let mut t = 0.0;
    let mut out = Vec::new();
    for i in 0..120 {
        out.push(t);
        t += [0.010, 0.017, 0.025, 0.014][i % 4];
    }
    out
}
fn fixed_time() -> SystemTime {
    time::OffsetDateTime::parse(
        "2026-09-03T12:00:01Z",
        &time::format_description::well_known::Rfc3339,
    )
    .expect("time")
    .into()
}

fn q_anchor(roll: f64, pitch: f64, yaw: f64) -> Quaternion {
    Quaternion::about_z(yaw)
        .mul(Quaternion::about_y(roll))
        .mul(Quaternion::about_x(-pitch))
        .normalized()
}
fn shortest_rotation_vector(q: Quaternion, dt: f64) -> Vector3 {
    if dt <= 0.0 {
        return vec3(0.0, 0.0, 0.0);
    }
    let q = q.normalized_shortest();
    let angle = 2.0 * q.w.clamp(-1.0, 1.0).acos();
    let s = (1.0 - q.w * q.w).sqrt();
    if s < 1e-12 {
        vec3(0.0, 0.0, 0.0)
    } else {
        vec3(
            q.x / s * angle / dt,
            q.y / s * angle / dt,
            q.z / s * angle / dt,
        )
    }
}
fn q_from_profile(q: &UnitQuaternion) -> Quaternion {
    Quaternion {
        w: q.w,
        x: q.x,
        y: q.y,
        z: q.z,
    }
    .normalized()
}

impl Quaternion {
    fn identity() -> Self {
        Self {
            w: 1.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
    fn about_x(angle: f64) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self {
            w: c,
            x: s,
            y: 0.0,
            z: 0.0,
        }
    }
    fn about_y(angle: f64) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self {
            w: c,
            x: 0.0,
            y: s,
            z: 0.0,
        }
    }
    fn about_z(angle: f64) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self {
            w: c,
            x: 0.0,
            y: 0.0,
            z: s,
        }
    }
    fn normalized(self) -> Self {
        let n = (self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        Self {
            w: self.w / n,
            x: self.x / n,
            y: self.y / n,
            z: self.z / n,
        }
    }
    fn normalized_shortest(self) -> Self {
        let q = self.normalized();
        if q.w < 0.0 {
            Self {
                w: -q.w,
                x: -q.x,
                y: -q.y,
                z: -q.z,
            }
        } else {
            q
        }
    }
    fn conjugate(self) -> Self {
        Self {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
    fn mul(self, o: Self) -> Self {
        Self {
            w: self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
            x: self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            y: self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            z: self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
        }
        .normalized()
    }
    fn rotate(self, v: Vector3) -> Vector3 {
        let qv = vec3(self.x, self.y, self.z);
        let t = scale(&cross(&qv, &v), 2.0);
        add(&v, &add(&scale(&t, self.w), &cross(&qv, &t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::{
        apply_calibration_to_angular, apply_calibration_to_gravity, apply_calibration_to_linear,
    };
    use crate::motion_filtering::estimator::{magnitude, sub};

    #[test]
    fn anchor_quaternion_convention_matches_gravity_signs() {
        let level = q_anchor(0.0, 0.0, 0.0)
            .conjugate()
            .rotate(vec3(0.0, 0.0, -1.0));
        assert_close(level.x, 0.0, 1e-12);
        assert_close(level.y, 0.0, 1e-12);
        assert_close(level.z, -1.0, 1e-12);
        assert!(
            q_anchor(0.2, 0.0, 0.0)
                .conjugate()
                .rotate(vec3(0.0, 0.0, -1.0))
                .x
                > 0.0
        );
        assert!(
            q_anchor(-0.2, 0.0, 0.0)
                .conjugate()
                .rotate(vec3(0.0, 0.0, -1.0))
                .x
                < 0.0
        );
        assert!(
            q_anchor(0.0, 0.2, 0.0)
                .conjugate()
                .rotate(vec3(0.0, 0.0, -1.0))
                .y
                > 0.0
        );
        assert!(
            q_anchor(0.0, -0.2, 0.0)
                .conjugate()
                .rotate(vec3(0.0, 0.0, -1.0))
                .y
                < 0.0
        );
        let yaw = q_anchor(0.0, 0.0, 1.0)
            .conjugate()
            .rotate(vec3(0.0, 0.0, -1.0));
        assert_close(yaw.z, -1.0, 1e-12);
    }

    #[test]
    fn suite_uses_raw_samples_and_b2_profiles() {
        let suite = synthetic_suite().unwrap();
        assert_eq!(suite.len(), 15);
        let identity = suite.iter().find(|f| f.name == "level_rest").unwrap();
        assert_eq!(identity.calibration_kind, "identity_b2");
        assert!(!identity.calibration_profile.yaw_calibrated);
        let calibrated = apply_calibration_to_gravity(
            &identity.calibration_profile,
            &identity.raw_samples[0].gravity_mps2,
        )
        .unwrap();
        assert_close(
            calibrated.z,
            identity.canonical_samples[0].gravity_mps2.z,
            1e-9,
        );
    }

    #[test]
    fn mounting_bias_fixture_requires_b2_to_recover_canonical_signal() {
        let suite = synthetic_suite().unwrap();
        let fixture = suite.iter().find(|f| f.name == "mounting_bias_b2").unwrap();
        let raw = &fixture.raw_samples[10];
        let canonical = &fixture.canonical_samples[10];
        assert!(magnitude(&sub(&raw.gravity_mps2, &canonical.gravity_mps2)) > 0.1);
        let g =
            apply_calibration_to_gravity(&fixture.calibration_profile, &raw.gravity_mps2).unwrap();
        let w =
            apply_calibration_to_angular(&fixture.calibration_profile, &raw.angular_velocity_rad_s)
                .unwrap();
        let l = apply_calibration_to_linear(
            &fixture.calibration_profile,
            &raw.linear_acceleration_mps2,
        )
        .unwrap();
        assert!(magnitude(&sub(&g, &canonical.gravity_mps2)) < 1e-6);
        assert!(magnitude(&sub(&w, &canonical.angular_velocity_rad_s)) < 1e-6);
        assert!(magnitude(&sub(&l, &canonical.linear_acceleration_mps2)) < 1e-6);
    }

    #[test]
    fn pure_yaw_truth_has_no_tilt_and_irregular_dt_is_real() {
        let suite = synthetic_suite().unwrap();
        let yaw = suite.iter().find(|f| f.name == "pure_yaw").unwrap();
        for truth in &yaw.truth {
            assert!(truth.roll_rad.abs() < 1e-12);
            assert!(truth.pitch_rad.abs() < 1e-12);
        }
        let irregular = suite
            .iter()
            .find(|f| f.name == "irregular_dt")
            .unwrap()
            .dt_seconds();
        assert!(irregular
            .windows(2)
            .any(|pair| (pair[0] - pair[1]).abs() > 1e-9));
    }

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected} +/- {tolerance}, got {actual}"
        );
    }
}
