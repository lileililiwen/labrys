# Design

RuntimeAdapter detects, prepares, builds, starts, stops, and health-checks a
project. The generic adapter consumes a Dockerfile, port, and healthcheck.
Node, .NET, Python, Rust, and Expo adapters add progressively richer detection.
Development and production profiles may use different commands, while OCI is
the production interchange boundary. Sandbox execution controls CPU, memory,
timeout, filesystem, network, capabilities, and process count.

