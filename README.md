# Anchor

**A cross-platform motion pipeline that turns Android sensor data into a reliable desktop stream.**

Anchor captures real motion samples on Android, sends them over the local network, and processes them in a Rust-powered desktop application. The project explores sensor calibration, real-time transport, motion filtering, and reproducible offline evaluation.

> Current status: active prototype. The data pipeline, diagnostics, calibration, and filter evaluation are implemented; authentication, encryption, automatic discovery, and the final visual overlay are still in progress.

## What it demonstrates

- End-to-end communication between an Android client and a desktop application
- A versioned JSON protocol shared across TypeScript and Rust
- UDP ingestion at 60 Hz with validation, ordering, session handling, backpressure, and diagnostics
- Offline motion-dataset recording, analysis, calibration, and deterministic filter evaluation
- Automated comparison of gravity-only, low-pass, and complementary tilt estimators
- A monorepo spanning mobile, desktop, shared packages, simulators, tests, and build tooling

## Architecture

```text
Android sensors
      |
      | MotionSampleV1 / UDP
      v
Rust receiver (Tauri)
      |
      +--> live diagnostics
      +--> dataset recorder
      +--> calibration pipeline
      +--> filter evaluation and selection
      v
React desktop UI
```

## Technology

- **Desktop:** Tauri, Rust, React, TypeScript, Vite
- **Mobile:** React Native, Android, Kotlin/Gradle
- **Protocol:** versioned JSON over UDP
- **Tooling:** pnpm workspaces, Cargo, Node.js test runner
- **Quality:** unit, integration, protocol, build, and dataset-fixture tests

## Repository structure

```text
apps/
  desktop/              Tauri desktop app and Rust motion pipeline
  mobile/               Android sensor capture and UDP sender
packages/
  protocol/             Shared protocol contracts
  motion-simulator/     Deterministic local stream simulator
docs/                   Technical notes and decisions
scripts/                Repeatable build and validation scripts
```

## Run the local vertical slice

### Prerequisites

- Node.js 20+
- pnpm
- Rust toolchain
- Android SDK and JDK 21 for the mobile build

Install dependencies:

```bash
pnpm install
```

Start the desktop receiver:

```bash
pnpm dev:desktop
```

In a second terminal, start the simulator:

```bash
pnpm dev:simulator -- --duration 5 --pattern sine
```

For the real Android-to-desktop flow, see:

- [Desktop guide](apps/desktop/README.md)
- [Mobile guide](apps/mobile/README.md)

## Verification

```bash
pnpm typecheck
pnpm protocol:test
pnpm protocol:test:rust
pnpm test:desktop
pnpm test:simulator
pnpm test:receiver
pnpm test:mobile:standalone-scripts
pnpm verify:mobile:bundle
```

## Security boundaries

The current prototype is designed for a trusted local network. UDP traffic is not authenticated or encrypted, and the receiver must not be exposed directly to the internet.

## Roadmap

- Apply the selected calibration and filter policy to the live stream
- Add authenticated and encrypted transport
- Add automatic desktop discovery on the LAN
- Build the final visual overlay and interaction layer

## Author

Built by [Thúlio Vicente](https://github.com/thuliovs) as an exploration of cross-platform systems, real-time data, and sensor-processing architecture.
