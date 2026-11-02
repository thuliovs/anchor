# Anchor

**An experimental motion-aware desktop system that turns Android sensor data into a low-latency visual reference.**

Anchor investigates whether a subtle visual reference synchronized with a vehicle's movement can help reduce the sensory conflict people may experience when using a computer in a car, bus, or train.

An Android phone captures motion at approximately 60 Hz and streams it over the local network. A Rust-powered desktop application validates and processes the data, estimates roll and pitch, exposes live diagnostics, and manages an experimental transparent overlay.

> [!IMPORTANT]
> Anchor is a personal engineering and research prototype, not a medical product. It does not currently claim to prevent or treat motion sickness, and the visual stimulus has not yet been validated with users or in moving vehicles.

## Why this project exists

Looking at a stationary screen while the vestibular system senses acceleration and turning can create conflicting signals. Anchor explores a simple hypothesis: a peripheral visual cue that follows real vehicle motion may provide a more coherent frame of reference without interrupting the user's work.

The project is also an end-to-end systems challenge involving mobile sensors, native modules, real-time networking, signal processing, desktop window management, measurement, and fail-safe behavior.

## Current status

Anchor is an active prototype with its core motion pipeline working on real hardware.

### Implemented and physically validated

- Native Android capture of linear acceleration, gravity, and angular velocity
- Versioned motion samples streamed at approximately 60 Hz over UDP
- Rust receiver with size limits, rate limiting, validation, ordering, and session handling
- Dataset recording, offline calibration, filter evaluation, and policy selection
- Fail-closed live roll/pitch estimation integrated with the desktop diagnostics
- Transport-integrity and latency-approximation measurement tools
- A transparent, always-on-top, click-through experimental overlay on Windows
- Standalone Android builds that run without Metro or `adb reverse`

### Still in progress

- Connecting processed motion to the visual overlay
- Rendering interpolation, amplitude limits, smoothing, and safe return to neutral
- Automatic desktop discovery, pairing, authentication, and encryption
- Product-ready packaging and broader Windows/Linux validation
- Vehicle studies and responsible evaluation of the original comfort hypothesis

## Architecture

```text
Android phone                                           Desktop
┌──────────────────────────────┐                       ┌───────────────────────────────┐
│ Motion sensors               │                       │ Rust / Tauri                  │
│ Kotlin aggregation (~60 Hz)  │  MotionSampleV1/UDP  │ Validate, order, rate-limit   │
│ React Native controller      ├──────────────────────►│ Calibrate and estimate tilt   │
│ Native UDP transport         │                       │ Measure and record datasets   │
└──────────────────────────────┘                       └───────────────┬───────────────┘
                                                                      │
                                                       ┌──────────────▼───────────────┐
                                                       │ React diagnostics            │
                                                       │ Experimental Windows overlay │
                                                       └──────────────────────────────┘
```

Each UDP datagram contains one complete, independent JSON sample. The system favors fresh motion over guaranteed delivery: under backpressure, it keeps the newest sample instead of building a queue of stale data.

## Engineering highlights

- **Shared contract:** a JSON Schema and common fixtures keep TypeScript and Rust validation aligned.
- **Native critical paths:** Android sensors and networking run in Kotlin; desktop ingestion and motion processing run in Rust.
- **Observable behavior:** both applications expose lifecycle, throughput, rejection, backpressure, and stream-health metrics.
- **Fail-closed processing:** invalid configuration, timing gaps, stale data, sequence errors, or estimator failures produce a neutral output.
- **Reproducible evaluation:** synthetic fixtures and captured datasets support deterministic comparison of gravity-only, low-pass, and complementary estimators.
- **Explicit measurement limits:** latency reports include clock-synchronization uncertainty and do not overstate sensor-to-screen performance.

## Technology

| Area | Stack |
| --- | --- |
| Desktop | Tauri 2, Rust, Tokio, React, TypeScript, Vite |
| Mobile | React Native, TypeScript, Android, Kotlin, TurboModules |
| Transport | Versioned JSON over UDP on a local network |
| Data and filtering | Rust CLIs, NDJSON datasets, offline calibration and estimator evaluation |
| Tooling | pnpm workspaces, Cargo, Gradle, Node.js test runner, Jest |

## Repository structure

```text
apps/
  desktop/              Tauri application, Rust receiver, processing, and overlay
  mobile/               React Native app and native Android sensor/UDP modules
packages/
  protocol/             Shared MotionSampleV1 contract, schema, and fixtures
tools/
  motion-simulator/     Deterministic local UDP stream simulator
docs/                   Architecture, experiments, decisions, and measurement methods
scripts/                Repeatable mobile build and verification tooling
```

## Run the local vertical slice

### Prerequisites

- Node.js 20+
- pnpm
- Rust toolchain and platform prerequisites for [Tauri 2](https://v2.tauri.app/start/prerequisites/)
- JDK 21 and the Android SDK only when building the mobile application

Install the workspace dependencies:

```bash
pnpm install
```

Start the desktop application:

```bash
pnpm dev:desktop
```

In a second terminal, send a five-second deterministic motion stream:

```bash
pnpm dev:simulator -- --duration 5 --pattern sine
```

The simulator sends samples to `127.0.0.1:57421`. For the physical Android-to-desktop flow, see the [mobile guide](apps/mobile/README.md) and [desktop guide](apps/desktop/README.md).

## Verification

The repository includes unit, integration, protocol-contract, native Android, Rust, lifecycle, and dataset-fixture coverage. The main workspace checks are:

```bash
pnpm typecheck
pnpm protocol:test
pnpm protocol:test:rust
pnpm test:desktop
pnpm test:simulator
pnpm test:receiver
pnpm --filter anchor-mobile test -- --runInBand
```

Android-native checks additionally require JDK 21 and the Android SDK. See the [project context](docs/PROJECT_CONTEXT.md#12-comandos-operacionais) for the complete build, measurement, and validation command set.

## Design constraints and safety

- The current UDP stream has no authentication or encryption. Use it only on a trusted local network and never expose port `57421` directly to the internet.
- The Android phone currently requires a documented mounting orientation; calibration corrects roll and pitch, but yaw remains unavailable.
- The overlay infrastructure is physically validated on Windows. Other windowing platforms remain unvalidated.
- A delayed or incorrect visual response could be ineffective or increase discomfort. Human testing requires explicit safety criteria and stop conditions.

## Documentation

- [Project context and current state](docs/PROJECT_CONTEXT.md)
- [Live motion pipeline](docs/LIVE_MOTION_PIPELINE_V1.md)
- [Calibration method](docs/MOTION_CALIBRATION_V1.md)
- [Filter evaluation](docs/MOTION_FILTER_EVALUATION_V1.md)
- [Transport measurement](docs/MOTION_TRANSPORT_MEASUREMENT_V1.md)
- [Latency measurement](docs/MOTION_LATENCY_MEASUREMENT_V1.md)
- [Motion protocol](packages/protocol/README.md)

## Author

Built by [Thúlio Vicente](https://github.com/thuliovs) as a hands-on exploration of cross-platform systems, real-time data pipelines, sensor processing, and responsible product experimentation.
