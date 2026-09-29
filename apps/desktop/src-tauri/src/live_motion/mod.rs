pub mod factory;
pub mod metrics;
pub mod prepared_calibration;
pub mod processor;

pub use crate::calibration::provenance::CalibrationProfileFingerprintV1;
pub use factory::{EstimatorFactory, FactoryError};
pub use metrics::{LiveMotionMetrics, OnlineScalarStats, OnlineScalarStatsSnapshot};
pub use prepared_calibration::PreparedCalibrationV1;
pub use processor::{
    LiveMotionPipeline, LiveMotionProcessor, LiveMotionState, LiveTiltSnapshotV1, NeutralReason,
    Tilt2, DISCONNECTED_AFTER, MAX_CONTIGUOUS_GAP, STALE_AFTER, WARM_UP_SAMPLE_COUNT,
};
