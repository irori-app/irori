# Postman Runtime Releases Simulation

This directory contains the Postman contract, mock configuration, and simulation suite used by the ignored `simulated_macos_releases` integration test in [`crates/runtime/src/manager.rs`](../../crates/runtime/src/manager.rs).

## Running Locally

Start the local mock server from the repository root:

```sh
postman simulation run tests/postman/simulations/runtime-releases.sim.yaml --port 4900 --output ndjson
```

Once the server prints its `listening` event, run the ignored test against the bound port:

```sh
IRORI_RELEASE_TEST_URL=http://127.0.0.1:4900 cargo test -p irori-runtime simulated_macos_releases -- --ignored
```

See [`tests/postman/simulations/runtime-releases.context.md`](simulations/runtime-releases.context.md) for what the simulation verifies and stops after Ctrl+C.
